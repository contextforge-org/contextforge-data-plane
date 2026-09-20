use super::{AuthorizedPrincipal, PrincipalConfig, PrincipalExtractor};
use tracing::instrument;

#[derive(Debug, Clone, Default)]
pub struct DefaultPrincipalExtractor {
    config: PrincipalConfig,
}

impl DefaultPrincipalExtractor {
    pub fn new(config: PrincipalConfig) -> Self {
        Self { config }
    }
}

impl PrincipalExtractor for DefaultPrincipalExtractor {
    #[instrument(name = "principal_extract", level = "info", skip_all)]
    fn extract(
        &self,
        claims: &serde_json::Value,
    ) -> Result<AuthorizedPrincipal, Box<dyn std::error::Error + Send + Sync>> {
        Ok(AuthorizedPrincipal::from_claims(claims, &self.config)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::{Permission, ScopeMapping, UserClaim};
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
    fn identity_profile_and_tenant_aliases_are_strict() {
        let mut claims = claims();
        let extractor =
            DefaultPrincipalExtractor::new(PrincipalConfig { user_claim: UserClaim::WoUserId, ..Default::default() });
        assert_eq!(extractor.extract(&claims).unwrap().user_id(), "watson-user");
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
    fn scopes_only_grant_permissions_when_configured() {
        let mut claims = claims();
        claims["role"] = "unknown".into();
        claims["scope"] = "aipg.admin unrelated".into();
        assert!(!DefaultPrincipalExtractor::default().extract(&claims).unwrap().has_permission(Permission::Admin));
        let mapping = ScopeMapping { admin: vec!["aipg.admin".into()], mcp_user: vec!["aipg.mcp".into()] };
        let mut config = PrincipalConfig { scope_mapping: Some(mapping), ..Default::default() };
        assert!(
            !DefaultPrincipalExtractor::new(config.clone()).extract(&claims).unwrap().has_permission(Permission::Admin)
        );
        config.scopes_only = true;
        assert!(
            DefaultPrincipalExtractor::new(config.clone()).extract(&claims).unwrap().has_permission(Permission::Admin)
        );
        claims["scope"] = "".into();
        claims["permissions"] = json!(["aipg.mcp"]);
        let principal = DefaultPrincipalExtractor::new(config.clone()).extract(&claims).unwrap();
        assert!(!principal.has_permission(Permission::Admin));
        assert!(principal.has_permission(Permission::MCPUser));
        assert_eq!(principal.scopes(), &["aipg.mcp"]);
        config.scopes_only = false;
        claims["role"] = "admin".into();
        assert!(
            !DefaultPrincipalExtractor::new(config.clone()).extract(&claims).unwrap().has_permission(Permission::Admin)
        );
        claims["permissions"] = json!([]);
        assert!(!DefaultPrincipalExtractor::new(config).extract(&claims).unwrap().has_permission(Permission::MCPUser));
    }

    #[test]
    fn malformed_permission_claims_are_rejected() {
        for (name, value) in [
            ("role", json!(["admin"])),
            ("roles", json!("admin")),
            ("roles", json!([42])),
            ("scope", json!(["aipg.admin"])),
            ("permissions", json!({"admin":true})),
        ] {
            let mut claims = claims();
            claims[name] = value;
            assert!(DefaultPrincipalExtractor::default().extract(&claims).is_err(), "{name}");
        }
    }

    #[test]
    fn debug_does_not_disclose_identity_or_scopes() {
        let principal = DefaultPrincipalExtractor::default().extract(&claims()).unwrap();
        let debug = format!("{principal:?}");
        for sensitive in ["watson", "subject", "tenant"] {
            assert!(!debug.contains(sensitive));
        }
    }
}
