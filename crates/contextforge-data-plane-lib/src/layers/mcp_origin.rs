use axum::{body::Body, middleware::Next, response::Response};
use http::{StatusCode, header};
use tracing::warn;
use url::{Origin, Url};

fn normalize_serialized_origin(raw: &str) -> Option<String> {
    let Ok(url) = Url::parse(&format!("{raw}/")) else {
        return None;
    };
    if url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match url.origin() {
        Origin::Tuple(scheme, host, port) => Some(format!("{scheme}://{host}:{port}")),
        Origin::Opaque(_) => None,
    }
}

fn forbidden_response() -> Response {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("Forbidden: Origin header is not allowed"))
        .expect("response should build")
}

/// Rejects non-serialized Origin values and normalizes valid values before
/// authentication.
///
/// RMCP owns the allowlist decision. This guard remains because RMCP 3.4
/// accepts path- and query-bearing values and does not normalize default ports.
pub async fn mcp_origin_syntax_layer(mut request: http::Request<Body>, next: Next) -> Response {
    let Some(origin) = request.headers().get(header::ORIGIN) else {
        return next.run(request).await;
    };
    let Ok(origin) = origin.to_str() else {
        warn!("mcp_origin_syntax_layer - rejected non-UTF-8 Origin header");
        return forbidden_response();
    };
    let Some(normalized) = normalize_serialized_origin(origin) else {
        warn!("mcp_origin_syntax_layer - rejected malformed Origin header origin = {origin}");
        return forbidden_response();
    };
    request
        .headers_mut()
        .insert(header::ORIGIN, normalized.parse().expect("normalized Origin is a valid header value"));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_serialized_tuple_origins_with_explicit_ports() {
        assert_eq!(
            normalize_serialized_origin("https://app.example.com").as_deref(),
            Some("https://app.example.com:443")
        );
        assert_eq!(normalize_serialized_origin("http://localhost:8080").as_deref(), Some("http://localhost:8080"));
        assert_eq!(normalize_serialized_origin("http://[::1]:8080").as_deref(), Some("http://[::1]:8080"));
    }

    #[test]
    fn rejects_non_serialized_origins() {
        for origin in [
            "null",
            "https://app.example.com/",
            "https://app.example.com/path",
            "https://app.example.com?query",
            "https://app.example.com#fragment",
            "https://user@app.example.com",
        ] {
            assert!(normalize_serialized_origin(origin).is_none(), "{origin} must be rejected");
        }
    }
}
