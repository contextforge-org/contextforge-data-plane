use tracing::instrument;

use crate::authorization::{AuthorizedPrincipal, PrincipalExtractor};

#[derive(Debug, Clone)]
pub struct DefaultPrincipalExtractor {}

impl PrincipalExtractor for DefaultPrincipalExtractor {
    #[instrument(name = "principal_extract", level = "info", skip_all)]
    fn extract(
        &self,
        claims: &serde_json::Value,
    ) -> Result<AuthorizedPrincipal, Box<dyn std::error::Error + Send + Sync>> {
        let user_id =
            ["sub", "user_id", "UserId"].into_iter().find_map(|claim| claims.get(claim)).and_then(|v| v.as_str());
        let tenant_id = ["tenantId", "tenant_id", "woTenantId"]
            .into_iter()
            .find_map(|claim| claims.get(claim))
            .and_then(|v| v.as_str());
        match (user_id, tenant_id) {
            (Some(user_id), Some(tenant_id)) => Ok(AuthorizedPrincipal::builder()
                .user_id(user_id.to_owned())
                .tenant_id(tenant_id.to_owned())
                .scopes(vec![])
                .build()),
            _ => Err("Can't create principal".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn supported_identity_aliases_are_preserved() {
        for user_claim in ["sub", "user_id", "UserId"] {
            for tenant_claim in ["tenantId", "tenant_id", "woTenantId"] {
                let claims = json!({user_claim: "user", tenant_claim: "tenant"});
                let principal = DefaultPrincipalExtractor {}.extract(&claims).unwrap();
                assert_eq!(principal.user_id, "user");
                assert_eq!(principal.tenant_id, "tenant");
                assert!(principal.scopes.is_empty());
            }
        }
    }

    #[test]
    fn existing_alias_precedence_is_preserved() {
        let claims = json!({
            "sub": "subject", "user_id": "alternate", "UserId": "other",
            "tenantId": "tenant", "tenant_id": "alternate", "woTenantId": "other"
        });
        let principal = DefaultPrincipalExtractor {}.extract(&claims).unwrap();
        assert_eq!(principal.user_id, "subject");
        assert_eq!(principal.tenant_id, "tenant");
    }

    #[test]
    fn missing_identity_is_rejected() {
        for claims in [
            json!({"sub": "user"}),
            json!({"tenant_id": "tenant"}),
            json!({"woUserId": "user", "woTenantId": "tenant"}),
        ] {
            assert!(DefaultPrincipalExtractor {}.extract(&claims).is_err());
        }
    }
}
