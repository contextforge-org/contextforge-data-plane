//! W3C trace-context propagation glue.
//!
//! This module wires the two request seams so the gateway continues the
//! caller's distributed trace and passes it on to backend MCP servers. All
//! functions here are no-ops unless [`init_tracing_logging`](crate::init_tracing_logging)
//! installs a global text-map propagator, so they are safe to call when
//! OpenTelemetry is disabled.

use std::{cell::RefCell, collections::HashMap, future::Future, hash::BuildHasher};

use axum::{extract::Request, middleware::Next, response::Response};
use opentelemetry::global;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::TraceContextExt;
use tower_http::trace::MakeSpan;
use tracing::{Span, field};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

pub const CORRELATION_ID_HEADER: http::HeaderName = http::HeaderName::from_static("x-correlation-id");
const MAX_CORRELATION_ID_LENGTH: usize = 255;

tokio::task_local! {
    static CURRENT_REQUEST_CONTEXT: RefCell<RequestObservabilityContext>;
}

/// Correlation metadata that follows one downstream HTTP request through the
/// dataplane from the first middleware onward.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestObservabilityContext {
    correlation_id: String,
}

impl RequestObservabilityContext {
    fn from_headers(headers: &http::HeaderMap) -> Self {
        let correlation_id = headers
            .get(&CORRELATION_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| is_valid_correlation_id(value))
            .map_or_else(|| Uuid::new_v4().simple().to_string(), ToOwned::to_owned);
        Self { correlation_id }
    }

    #[must_use]
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }
}

fn is_valid_correlation_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CORRELATION_ID_LENGTH
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

async fn scope_request_context<F>(context: RequestObservabilityContext, future: F) -> F::Output
where
    F: Future,
{
    CURRENT_REQUEST_CONTEXT.scope(RefCell::new(context), future).await
}

#[must_use]
pub fn current_request_context() -> Option<RequestObservabilityContext> {
    CURRENT_REQUEST_CONTEXT.try_with(|context| context.borrow().clone()).ok()
}

/// Establishes request-scoped correlation before tracing and the application
/// middleware run, then echoes the same ID on the downstream response.
pub async fn request_context_layer(mut request: Request, next: Next) -> Response {
    let context = RequestObservabilityContext::from_headers(request.headers());
    request.extensions_mut().insert(context.clone());

    scope_request_context(context, async move {
        let mut response = next.run(request).await;
        if let Some(context) = current_request_context()
            && let Ok(value) = http::HeaderValue::from_str(context.correlation_id())
        {
            response.headers_mut().insert(CORRELATION_ID_HEADER, value);
        }
        response
    })
    .await
}

/// Reads inbound `http::HeaderMap` for the text-map propagator (extract side).
struct HeaderExtractor<'a>(&'a http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(http::HeaderName::as_str).collect()
    }
}

/// Writes the outbound backend header map for the text-map propagator (inject
/// side). Malformed keys/values are dropped rather than propagated.
struct HeaderInjector<'a, S>(&'a mut HashMap<http::HeaderName, http::HeaderValue, S>);

impl<S: BuildHasher> Injector for HeaderInjector<'_, S> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(name), Ok(value)) =
            (http::HeaderName::from_bytes(key.as_bytes()), http::HeaderValue::from_str(&value))
        {
            self.0.insert(name, value);
        }
    }
}

/// [`MakeSpan`] that opens the per-request span and re-parents it onto any W3C
/// trace context found in the inbound headers, so the gateway span continues
/// the caller's trace instead of starting a fresh one.
///
/// Body-generic on purpose: the outermost `TraceLayer` sees whatever body type
/// the server hands it, and a struct impl avoids pinning that down.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExtractingMakeSpan;

impl<B> MakeSpan<B> for ExtractingMakeSpan {
    fn make_span(&mut self, request: &http::Request<B>) -> Span {
        let correlation_id = request
            .extensions()
            .get::<RequestObservabilityContext>()
            .map_or("", RequestObservabilityContext::correlation_id);
        let span = tracing::info_span!(
            "http-request",
            transaction_id = correlation_id,
            correlation_id,
            trace_id = field::Empty,
            span_id = field::Empty,
            method = %request.method(),
            path = %request.uri().path(),
            version = ?request.version(),
        );
        let parent =
            global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(request.headers())));
        // Errors only when no OTel layer is registered (OTel disabled); ignore.
        let _ = span.set_parent(parent);
        let span_context = span.context().span().span_context().clone();
        if span_context.is_valid() {
            span.record("trace_id", span_context.trace_id().to_string());
            span.record("span_id", span_context.span_id().to_string());
        }
        span
    }
}

/// Injects the request correlation ID and current span context into the
/// backend request headers. Trace propagation is a no-op when there is no
/// valid active OpenTelemetry context; request correlation is independent.
pub fn inject_current_context<S: BuildHasher>(
    headers: &mut HashMap<http::HeaderName, http::HeaderValue, S>,
    request_context: Option<&RequestObservabilityContext>,
) {
    if let Some(request_context) = request_context
        && let Ok(value) = http::HeaderValue::from_str(request_context.correlation_id())
    {
        headers.insert(CORRELATION_ID_HEADER, value);
    }
    let context = Span::current().context();
    global::get_text_map_propagator(|propagator| propagator.inject_context(&context, &mut HeaderInjector(headers)));
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::to_bytes, extract::Extension, middleware, routing::get};
    use http::{Request, StatusCode};
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use tower::ServiceExt;

    use super::*;

    async fn correlation_handler(Extension(context): Extension<RequestObservabilityContext>) -> String {
        assert_eq!(current_request_context(), Some(context.clone()));
        context.correlation_id().to_owned()
    }

    fn correlation_app() -> Router {
        Router::new().route("/", get(correlation_handler)).layer(middleware::from_fn(request_context_layer))
    }

    #[tokio::test]
    async fn preserves_correlation_id_in_context_and_response() {
        let request = Request::builder()
            .uri("/")
            .header(CORRELATION_ID_HEADER, "request-123")
            .body(axum::body::Body::empty())
            .expect("request should build");

        let response = correlation_app().oneshot(request).await.expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CORRELATION_ID_HEADER], "request-123");
        let body =
            to_bytes(response.into_body(), MAX_CORRELATION_ID_LENGTH).await.expect("response body should be readable");
        assert_eq!(&body[..], b"request-123");
    }

    #[tokio::test]
    async fn generates_correlation_id_when_incoming_value_is_invalid() {
        let request = Request::builder()
            .uri("/")
            .header(CORRELATION_ID_HEADER, "not a safe correlation id")
            .body(axum::body::Body::empty())
            .expect("request should build");

        let response = correlation_app().oneshot(request).await.expect("request should succeed");
        let generated = response.headers()[CORRELATION_ID_HEADER].to_str().expect("generated ID should be ASCII");

        assert_eq!(generated.len(), 32);
        assert!(generated.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn concurrent_requests_keep_correlation_context_isolated() {
        let app = correlation_app();
        let request = |correlation_id| {
            Request::builder()
                .uri("/")
                .header(CORRELATION_ID_HEADER, correlation_id)
                .body(axum::body::Body::empty())
                .expect("request should build")
        };

        let (first, second) =
            tokio::join!(app.clone().oneshot(request("request-a")), app.oneshot(request("request-b")));

        assert_eq!(first.expect("first request should succeed").headers()[CORRELATION_ID_HEADER], "request-a");
        assert_eq!(second.expect("second request should succeed").headers()[CORRELATION_ID_HEADER], "request-b");
    }

    #[tokio::test]
    async fn injects_scoped_correlation_id_without_trace_export() {
        let context = RequestObservabilityContext { correlation_id: "request-123".to_owned() };

        let headers = scope_request_context(context, async {
            let mut headers = HashMap::new();
            inject_current_context(&mut headers, current_request_context().as_ref());
            headers
        })
        .await;

        assert_eq!(headers[&CORRELATION_ID_HEADER], "request-123");
    }

    #[test]
    fn round_trips_traceparent_through_extract_and_inject() {
        // A well-formed W3C traceparent carrying a known trace id.
        let trace_id = "0af7651916cd43dd8448eb211c80319c";
        let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

        let mut inbound = http::HeaderMap::new();
        inbound.insert("traceparent", http::HeaderValue::from_str(&traceparent).unwrap());

        let propagator = TraceContextPropagator::new();
        let parent = propagator.extract(&HeaderExtractor(&inbound));

        let mut outbound = HashMap::new();
        propagator.inject_context(&parent, &mut HeaderInjector(&mut outbound));

        let injected = outbound.get(&http::HeaderName::from_static("traceparent")).expect("traceparent injected");
        // Same trace id must survive extract -> inject (span id differs).
        assert!(injected.to_str().unwrap().contains(trace_id));
    }
}
