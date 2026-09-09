use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, OriginalUri, Request, State},
    middleware::Next,
    response::Response,
};
use contextforge_data_plane_cpex::{GatewayPluginRuntimeHandle, HttpHookPayload};
use cpex::cpex_core::extensions::{Extensions, HttpExtension, RequestExtension, SecurityExtension};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use opentelemetry::trace::TraceContextExt;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::errors::custom_error;

pub(crate) async fn http_plugin_layer(
    State(runtime): State<GatewayPluginRuntimeHandle>,
    mut request: Request,
    next: Next,
) -> Response {
    let trace = tracing::Span::current().context();
    let span = trace.span();
    let context = span.span_context();
    let uri = request.extensions().get::<OriginalUri>().map_or(request.uri(), |uri| &uri.0);
    let payload = HttpHookPayload {
        method: request.method().to_string(),
        path: uri.path().to_owned(),
        client_addr: request.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0),
        status_code: None,
    };
    let extensions = Extensions {
        request: Some(Arc::new(RequestExtension {
            request_id: Some(uuid::Uuid::new_v4().to_string()),
            trace_id: context.is_valid().then(|| context.trace_id().to_string()),
            span_id: context.is_valid().then(|| context.span_id().to_string()),
            ..Default::default()
        })),
        http: Some(Arc::new(HttpExtension {
            method: Some(payload.method.clone()),
            path: Some(payload.path.clone()),
            host: request.headers().get(http::header::HOST).and_then(|value| value.to_str().ok()).map(str::to_owned),
            scheme: uri.scheme_str().map(str::to_owned),
            request_headers: header_values(request.headers()),
            ..Default::default()
        })),
        security: Some(Arc::new(SecurityExtension::default())),
        ..Default::default()
    };
    let state = match runtime.before_http_request(payload, extensions).await {
        Ok(state) => state,
        Err(error) => {
            tracing::warn!(%error, "HTTP plugin configuration is unavailable");
            return custom_error(StatusCode::INTERNAL_SERVER_ERROR, "Runtime plugin configuration is unavailable");
        },
    };
    let updated = state.request.extensions().await;
    if let Some(http) = updated.http {
        apply_headers(request.headers_mut(), &http.request_headers);
    }
    state.request.sync_request_headers(header_values(request.headers())).await;
    request.extensions_mut().insert(state.request.clone());
    let mut response = next.run(request).await;
    // Headers are still mutable here. Do not collect or wrap the response body:
    // streaming MCP operations may continue after this hook has run.
    let updated = state.after_http_request(response.status().as_u16(), header_values(response.headers())).await;
    if let Some(http) = updated.http {
        apply_headers(response.headers_mut(), &http.response_headers);
    }
    response
}

fn header_values(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.to_string(), value.to_owned())))
        .collect()
}

fn apply_headers(headers: &mut HeaderMap, updates: &HashMap<String, String>) {
    // Validate the entire update first. Preserve repeated and non-UTF8 headers
    // unless a plugin actually changes their value; the built-in hooks merge edits.
    let parsed = updates
        .iter()
        .map(|(name, value)| {
            Some((HeaderName::try_from(name.as_str()).ok()?, HeaderValue::try_from(value.as_str()).ok()?))
        })
        .collect::<Option<Vec<_>>>();
    let Some(parsed) = parsed else {
        tracing::warn!("HTTP plugin returned invalid headers; ignoring header changes");
        return;
    };
    for (name, value) in parsed {
        if headers.get_all(&name).iter().next_back() != Some(&value) {
            headers.insert(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_updates_preserve_repeated_and_binary_headers_and_validate_atomically() {
        let mut headers = HeaderMap::new();
        headers.append("set-cookie", HeaderValue::from_static("first=1"));
        headers.append("set-cookie", HeaderValue::from_static("second=2"));
        headers.insert("x-binary", HeaderValue::from_bytes(&[0xff]).expect("opaque header"));
        let mut updates = header_values(&headers);
        updates.insert("x-plugin".to_owned(), "added".to_owned());
        apply_headers(&mut headers, &updates);
        assert_eq!(headers.get_all("set-cookie").iter().count(), 2);
        assert_eq!(headers["x-binary"].as_bytes(), &[0xff]);
        assert_eq!(headers["x-plugin"], "added");
        let before = headers.clone();
        updates.insert("x-plugin".to_owned(), "changed".to_owned());
        updates.insert("x-invalid".to_owned(), "invalid\r\nvalue".to_owned());
        apply_headers(&mut headers, &updates);
        assert_eq!(headers, before);
    }
}
