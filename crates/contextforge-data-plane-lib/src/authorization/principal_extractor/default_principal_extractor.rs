use super::{AuthorizedPrincipal, PrincipalExtractor};
use tracing::instrument;

#[derive(Debug, Clone, Default)]
pub struct DefaultPrincipalExtractor {}

impl PrincipalExtractor for DefaultPrincipalExtractor {
    #[instrument(name = "principal_extract", level = "info", skip_all)]
    fn extract(
        &self,
        claims: &serde_json::Value,
    ) -> Result<AuthorizedPrincipal, Box<dyn std::error::Error + Send + Sync>> {
        Ok(AuthorizedPrincipal::from_claims(claims)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::Permission;
    use serde_json::{Value, json};

    fn claims() -> Value {
        json!({"iss":"watson", "sub":"subject", "woUserId":"watson-user", "woTenantId":"tenant", "role":"user"})
    }

    #[test]
    fn role_permissions_are_explicit_and_admin_includes_mcp() {
        for (role, admin, mcp) in [
            ("admin", true, true),
            ("builder", false, true),
            ("user", false, true),
            ("unknown", false, false),
            ("Admin", false, false),
            ("", false, false),
        ] {
            let mut claims = claims();
            claims["role"] = role.into();
            let principal = DefaultPrincipalExtractor::default().extract(&claims).unwrap();
            assert_eq!(principal.has_permission(Permission::Admin), admin, "{role}");
            assert_eq!(principal.has_permission(Permission::MCPUser), mcp, "{role}");
            assert_eq!(
                (principal.issuer(), principal.user_id(), principal.tenant_id()),
                ("watson", "subject", "tenant")
            );
        }
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("role");
        assert!(!DefaultPrincipalExtractor::default().extract(&claims).unwrap().has_permission(Permission::MCPUser));
        claims["roles"] = json!(["unknown", "admin"]);
        assert!(DefaultPrincipalExtractor::default().extract(&claims).unwrap().has_permission(Permission::Admin));
    }

    #[test]
    fn subject_and_tenant_aliases_are_required() {
        let mut claims = claims();
        let extractor = DefaultPrincipalExtractor {};
        assert_eq!(extractor.extract(&claims).unwrap().user_id(), "subject");
        let mut missing_sub = claims.clone();
        missing_sub.as_object_mut().unwrap().remove("sub");
        assert!(extractor.extract(&missing_sub).is_err());
        claims["tenant_id"] = "tenant".into();
        assert!(extractor.extract(&claims).is_ok());
        claims["tenantId"] = "different".into();
        assert!(extractor.extract(&claims).is_err());
        for invalid in [Value::Null, json!(42), json!(""), json!("  ")] {
            for claim in ["sub", "woTenantId", "iss"] {
                let mut claims = super::tests::claims();
                claims[claim] = invalid.clone();
                assert!(DefaultPrincipalExtractor::default().extract(&claims).is_err(), "{claim}");
            }
        }
        for tenant in ["woTenantId", "tenant_id", "tenantId"] {
            let claims = json!({"iss":"watson", "sub":"subject", tenant:"tenant", "role":"user"});
            assert!(DefaultPrincipalExtractor::default().extract(&claims).is_ok());
        }
    }

    #[test]
    fn malformed_permission_claims_are_rejected() {
        for (name, value) in [("role", json!(["admin"])), ("roles", json!("admin")), ("roles", json!([42]))] {
            let mut claims = claims();
            claims[name] = value;
            assert!(DefaultPrincipalExtractor::default().extract(&claims).is_err(), "{name}");
        }
    }

    #[test]
    fn debug_does_not_disclose_identity() {
        let principal = DefaultPrincipalExtractor::default().extract(&claims()).unwrap();
        let debug = format!("{principal:?}");
        for sensitive in ["watson", "subject", "tenant"] {
            assert!(!debug.contains(sensitive));
        }
    }
}
