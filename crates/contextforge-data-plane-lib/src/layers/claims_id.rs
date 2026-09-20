use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::{
    AuthenticationError,
    common::ContextForgeDataPlaneAppState,
    errors::{custom_error, unauthorized_response},
};

pub async fn claims_layer(
    State(state): State<ContextForgeDataPlaneAppState>,
    request: http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let (mut parts, body) = request.into_parts();

    let mut authorizations = parts.headers.get_all(http::header::AUTHORIZATION).iter();
    let Some(authorization) = authorizations.next() else {
        return unauthorized_response("Missing bearer token");
    };
    if authorizations.next().is_some() {
        return unauthorized_response("Ambiguous bearer token");
    }
    let claims = match state.authorization_service.authorize(authorization).await {
        Ok(claims) => claims,
        Err(AuthenticationError::InvalidToken) => return unauthorized_response("Invalid bearer token"),
        Err(AuthenticationError::KeysUnavailable) => {
            return custom_error(http::StatusCode::SERVICE_UNAVAILABLE, "Authentication temporarily unavailable");
        },
    };
    parts.extensions.insert(claims);
    let request = Request::from_parts(parts, body);
    next.run(request).await
}
