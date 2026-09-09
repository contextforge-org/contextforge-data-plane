mod cel_principal_extractor;
mod default_principal_extractor;
use contextforge_data_plane_apis::User;
use typed_builder::TypedBuilder;

pub use cel_principal_extractor::CelPrincipalExtractor;
pub use default_principal_extractor::DefaultPrincipalExtractor;

#[derive(Debug, Clone, TypedBuilder)]
#[allow(dead_code)]
pub struct AuthorizedPrincipal {
    user_id: String,
    tenant_id: String,
    scopes: Vec<String>,
}

impl AuthorizedPrincipal {
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
}

impl<'a> From<&'a AuthorizedPrincipal> for User<'a> {
    fn from(value: &'a AuthorizedPrincipal) -> Self {
        Self::new(&value.user_id)
    }
}

pub trait PrincipalExtractor {
    fn extract(
        &self,
        claims: &serde_json::Value,
    ) -> Result<AuthorizedPrincipal, Box<dyn std::error::Error + Send + Sync>>;
}
