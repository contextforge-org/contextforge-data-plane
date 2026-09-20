use crate::{
    authorization::{AuthorizedPrincipal, Permission},
    errors::{custom_error, unauthorized_response},
};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http::StatusCode;

/// Reusable API-level guard. Install after verified principal extraction and before
/// configuration or backend access. Use `Admin` for future management routes.
pub async fn require_permission(State(permission): State<Permission>, request: Request, next: Next) -> Response {
    let Some(principal) = request.extensions().get::<AuthorizedPrincipal>() else {
        return unauthorized_response("Missing verified identity");
    };
    if !principal.has_permission(permission) {
        return custom_error(StatusCode::FORBIDDEN, "Insufficient permission");
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::{DefaultPrincipalExtractor, PrincipalExtractor};
    use axum::{Router, body::Body, middleware, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn guards_require_a_principal_and_the_requested_permission() {
        for permission in [Permission::Admin, Permission::MCPUser] {
            let app = Router::new()
                .route("/", get(|| async { StatusCode::NO_CONTENT }))
                .layer(middleware::from_fn_with_state(permission, require_permission));
            let response = app.clone().oneshot(Request::new(Body::empty())).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            for role in ["admin", "builder", "user", "unknown"] {
                let principal = DefaultPrincipalExtractor::default()
                    .extract(&serde_json::json!({"iss":"watson","sub":"user","tenant_id":"tenant","role":role}))
                    .unwrap();
                let mut request = Request::new(Body::empty());
                request.extensions_mut().insert(principal);
                let response = app.clone().oneshot(request).await.unwrap();
                let allowed =
                    role == "admin" || permission == Permission::MCPUser && ["builder", "user"].contains(&role);
                assert_eq!(response.status(), if allowed { StatusCode::NO_CONTENT } else { StatusCode::FORBIDDEN });
            }
        }
    }
}
