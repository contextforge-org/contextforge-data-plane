use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;

use http::HeaderValue;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use crate::JwksConfig;

mod jwks;
mod principal_extractor;

pub use principal_extractor::{
    AuthorizedPrincipal, CelPrincipalExtractor, DefaultPrincipalExtractor, PrincipalExtractor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    Admin,
    MCPUser,
}

pub fn get_authorization_service(
    config: &JwksConfig,
) -> Result<Arc<dyn AuthorizationService + Send + Sync>, AuthorizationError> {
    let service = jwks::JwtAuthorizationService::from_jwks_url(
        config.url.clone(),
        config.ca_cert_path.as_ref(),
        config.issuer.clone(),
        config.audiences.clone(),
    )?;
    Ok(Arc::new(service) as Arc<dyn AuthorizationService + Send + Sync>)
}

#[async_trait]
pub trait AuthorizationService: std::fmt::Debug {
    async fn authorize(&self, authorization_token: &HeaderValue) -> Option<AuthorizationClaims>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthenticationError {
    #[error("invalid bearer token")]
    InvalidToken,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AuthorizationError {
    #[error("JWT trust configuration requires a nonempty issuer and audience")]
    InvalidTrustConfiguration,
    #[error("SaaS JWKS contains no supported signing keys")]
    NoSupportedKeys,
    #[error("SaaS JWKS is invalid")]
    InvalidJson(#[source] serde_json::Error),
    #[error("SaaS JWKS is invalid")]
    InvalidKey(#[source] jsonwebtoken::errors::Error),

    #[error("MCPOPS_JWKS_URL must use HTTPS (HTTP is allowed only for loopback testing)")]
    InsecureJwksUrl,
    #[error("unable to retrieve SaaS JWKS")]
    JwksRequest(#[source] reqwest::Error),
    #[error("unable to read JWKS CA certificate `{path}`")]
    ReadJwksCaCertificate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JWKS CA certificate `{path}` is invalid")]
    InvalidJwksCaCertificate {
        path: PathBuf,
        #[source]
        source: reqwest::Error,
    },
    #[error("JWKS CA certificate `{path}` contains no certificates")]
    EmptyJwksCaCertificate { path: PathBuf },
    #[error("SaaS JWKS response exceeds 1 MiB")]
    JwksResponseTooLarge,
}

#[derive(Clone, Debug, Serialize, Deserialize, TypedBuilder, PartialEq)]
pub struct User {
    pub user_id: String,
    pub tenant_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TypedBuilder, PartialEq)]
pub struct Scopes {
    server_id: Option<String>,
    permissions: Vec<String>,
    ip_restrictions: Vec<String>,
    time_restrictions: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TypedBuilder, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Idp {
    real_name: String,
    iss: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, TypedBuilder)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationClaims {
    value: serde_json::Value,
}

impl AuthorizationClaims {
    pub fn as_value(&self) -> &serde_json::Value {
        &self.value
    }
}

impl std::fmt::Debug for AuthorizationClaims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizationClaims").finish_non_exhaustive()
    }
}

impl From<serde_json::Value> for AuthorizationClaims {
    fn from(value: serde_json::Value) -> Self {
        Self { value }
    }
}

impl From<&AuthorizationClaims> for serde_json::Value {
    fn from(val: &AuthorizationClaims) -> Self {
        val.value.clone()
    }
}

impl From<AuthorizationClaims> for serde_json::Value {
    fn from(val: AuthorizationClaims) -> Self {
        val.value
    }
}
