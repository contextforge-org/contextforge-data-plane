use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;

use http::HeaderValue;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use crate::Config;

mod jwks;
mod principal_extractor;

pub use principal_extractor::{
    AuthorizedPrincipal, CelPrincipalExtractor, DefaultPrincipalExtractor, PrincipalExtractor,
};

pub fn get_authorization_service(
    config: &Config,
) -> Result<Arc<dyn AuthorizationService + Send + Sync>, AuthorizationError> {
    let service =
        jwks::JwtAuthorizationService::from_jwks_url(config.jwks_url.clone(), config.jwks_ca_cert_path.as_ref())?;
    Ok(Arc::new(service) as Arc<dyn AuthorizationService + Send + Sync>)
}

#[async_trait]
pub trait AuthorizationService: std::fmt::Debug {
    async fn authorize(&self, authorization_token: &HeaderValue) -> Option<AuthorizationClaims>;
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AuthorizationError {
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, TypedBuilder)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationClaims {
    value: serde_json::Value,
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
