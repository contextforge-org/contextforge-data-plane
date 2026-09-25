use crate::{
    authorization::{AuthenticationError, AuthorizationClaims, AuthorizedPrincipal, Permission},
    errors::{custom_error, unauthorized_response},
};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http::StatusCode;

pub async fn require_permission(State(permission): State<Permission>, request: Request, next: Next) -> Response {
    if request.extensions().get::<AuthorizedPrincipal>().is_none() {
        return unauthorized_response("Missing verified identity");
    }
    let Some(claims) = request.extensions().get::<AuthorizationClaims>() else {
        return unauthorized_response("Missing verified claims");
    };
    match has_permission(claims, permission) {
        Ok(true) => next.run(request).await,
        Ok(false) => custom_error(StatusCode::FORBIDDEN, "Insufficient permission"),
        Err(_) => unauthorized_response("Invalid role claims"),
    }
}

/// Test role mapping; awaiting confirmation from WxO.
fn has_permission(claims: &AuthorizationClaims, permission: Permission) -> Result<bool, AuthenticationError> {
    let allows = |role: &str| match role {
        "admin" => true,
        "builder" | "user" => permission == Permission::MCPUser,
        _ => false,
    };
    let claims = claims.as_value();
    let mut granted = false;
    if let Some(roles) = claims.get("roles") {
        for role in roles.as_array().ok_or(AuthenticationError::InvalidToken)? {
            granted |= allows(role.as_str().ok_or(AuthenticationError::InvalidToken)?);
        }
    }
    if let Some(role) = claims.get("role") {
        granted |= allows(role.as_str().ok_or(AuthenticationError::InvalidToken)?);
    }
    Ok(granted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::{CelPrincipalExtractor, DefaultPrincipalExtractor, PrincipalExtractor};
    use axum::{Router, body::Body, middleware, routing::get};
    use serde_json::json;
    use tower::ServiceExt;

    fn app(permission: Permission) -> Router {
        Router::new()
            .route("/", get(|| async { StatusCode::NO_CONTENT }))
            .layer(middleware::from_fn_with_state(permission, require_permission))
    }

    #[tokio::test]
    async fn guards_require_both_verified_claims_and_identity() {
        let claims = json!({"sub":"user", "tenant_id":"tenant", "roles":["admin"]});
        let principal = DefaultPrincipalExtractor {}.extract(&claims).unwrap();
        for (include_claims, include_principal) in [(false, false), (true, false), (false, true)] {
            let mut request = Request::new(Body::empty());
            if include_claims {
                request.extensions_mut().insert(AuthorizationClaims::from(claims.clone()));
            }
            if include_principal {
                request.extensions_mut().insert(principal.clone());
            }
            let response = app(Permission::MCPUser).oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn guards_map_verified_roles_to_the_requested_permission() {
        use StatusCode as S;
        for (role_claims, admin_status, mcp_status) in [
            (json!({"roles":["admin"]}), S::NO_CONTENT, S::NO_CONTENT),
            (json!({"roles":["builder"]}), S::FORBIDDEN, S::NO_CONTENT),
            (json!({"roles":["user"]}), S::FORBIDDEN, S::NO_CONTENT),
            (json!({"role":"admin"}), S::NO_CONTENT, S::NO_CONTENT),
            (json!({"roles":["user"], "role":"admin"}), S::NO_CONTENT, S::NO_CONTENT),
            (json!({"roles":["unknown", "user"]}), S::FORBIDDEN, S::NO_CONTENT),
            (json!({"roles":["ServiceAdmin"]}), S::FORBIDDEN, S::FORBIDDEN),
            (json!({"roles":[]}), S::FORBIDDEN, S::FORBIDDEN),
            (json!({}), S::FORBIDDEN, S::FORBIDDEN),
            (json!({"roles":"admin"}), S::UNAUTHORIZED, S::UNAUTHORIZED),
            (json!({"role":["admin"]}), S::UNAUTHORIZED, S::UNAUTHORIZED),
            (json!({"roles":["admin", 42]}), S::UNAUTHORIZED, S::UNAUTHORIZED),
            (json!({"roles":["admin"], "role":42}), S::UNAUTHORIZED, S::UNAUTHORIZED),
        ] {
            let mut claims = json!({"sub":"user", "tenant_id":"tenant"});
            claims.as_object_mut().unwrap().extend(role_claims.as_object().unwrap().clone());
            let principal = DefaultPrincipalExtractor {}.extract(&claims).unwrap();
            for (permission, expected) in [(Permission::Admin, admin_status), (Permission::MCPUser, mcp_status)] {
                let mut request = Request::new(Body::empty());
                request.extensions_mut().insert(principal.clone());
                request.extensions_mut().insert(AuthorizationClaims::from(claims.clone()));
                let response = app(permission).oneshot(request).await.unwrap();
                assert_eq!(response.status(), expected, "{permission:?}: {role_claims}");
            }
        }
    }

    #[tokio::test]
    async fn cel_identity_mapping_cannot_grant_roles_absent_from_the_token() {
        let extractor = CelPrincipalExtractor::from_expression(
            r#"{"user_id": claims.sub, "tenant_id": claims.woTenantId, "role": "admin", "scopes": ["Admin"]}"#,
        )
        .unwrap();
        for (roles, mcp_status) in [(json!(["user"]), StatusCode::NO_CONTENT), (json!([]), StatusCode::FORBIDDEN)] {
            let claims = json!({"sub":"user", "woTenantId":"tenant", "roles":roles});
            let principal = extractor.extract(&claims).unwrap();
            for (permission, expected) in
                [(Permission::Admin, StatusCode::FORBIDDEN), (Permission::MCPUser, mcp_status)]
            {
                let mut request = Request::new(Body::empty());
                request.extensions_mut().insert(principal.clone());
                request.extensions_mut().insert(AuthorizationClaims::from(claims.clone()));
                let response = app(permission).oneshot(request).await.unwrap();
                assert_eq!(response.status(), expected);
            }
        }
    }
}
