use crate::layers::AuthorizedPrincipal;

#[derive(Debug, Clone)]
pub struct DefaultPrincipalExtractor {}

pub trait PrincipalExtractor {
    fn extract(
        &self,
        claims: &serde_json::Value,
    ) -> Result<AuthorizedPrincipal, Box<dyn std::error::Error + Send + Sync>>;
}

impl PrincipalExtractor for DefaultPrincipalExtractor {
    fn extract(
        &self,
        claims: &serde_json::Value,
    ) -> Result<AuthorizedPrincipal, Box<dyn std::error::Error + Send + Sync>> {
        let user_id =
            ["sub", "user_id", "UserId"].into_iter().find_map(|claim| claims.get(claim)).and_then(|v| v.as_str());
        let tenant_id =
            ["tenantId", "tenant_id"].into_iter().find_map(|claim| claims.get(claim)).and_then(|v| v.as_str());
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
