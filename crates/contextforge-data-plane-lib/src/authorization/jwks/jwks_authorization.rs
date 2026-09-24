use crate::JwksConfig;
use crate::authorization::jwks::jwks::Jwks;
use crate::authorization::{AuthorizationClaims, AuthorizationError, AuthorizationService};
use async_trait::async_trait;
use jsonwebtoken::decode_header;
use std::fmt;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;
use tracing::instrument;
use url::Url;

const JWKS_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const JWKS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const JWKS_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub struct JwtAuthorizationService {
    jwks: Jwks,
}

impl JwtAuthorizationService {
    pub fn new(config: &JwksConfig) -> Result<Self, AuthorizationError> {
        let validation = Jwks::validation(config)?;
        let url = parse_jwks_url(config.url.clone())?;
        let mut client = reqwest::Client::builder()
            .tls_backend_rustls()
            .connect_timeout(JWKS_CONNECT_TIMEOUT)
            .read_timeout(JWKS_READ_TIMEOUT)
            .timeout(JWKS_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("mcp-ops/", env!("CARGO_PKG_VERSION")));
        if let Some(ca_cert_path) = &config.ca_cert_path {
            client = client.tls_certs_only(load_ca_certificates(ca_cert_path)?);
        }
        let client = client.build().map_err(AuthorizationError::JwksRequest)?;
        Ok(Self { jwks: Jwks::new(client, url, validation) })
    }

    async fn authorize_token(&self, token: &str) -> Option<AuthorizationClaims> {
        let header = decode_header(token).ok()?;
        self.jwks.validate(token, &header).await.ok()
    }
}

impl fmt::Debug for JwtAuthorizationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JwtAuthorizationService")
            .field("verification_source", &"remote JWKS")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl AuthorizationService for JwtAuthorizationService {
    #[instrument(name = "jwt_authorization_service", level = "info", skip_all)]
    async fn authorize(&self, authorization_token: &http::HeaderValue) -> Option<AuthorizationClaims> {
        let token = authorization_token.as_bytes().strip_prefix(b"Bearer ")?;
        let token = str::from_utf8(token).ok()?;
        let claims = self.authorize_token(token).await;

        if claims.is_none() {
            tracing::debug!("validate_saas_jwt  SaaS JWT was rejected");
        }

        claims
    }
}

fn parse_jwks_url(url: Url) -> Result<Url, AuthorizationError> {
    let secure = url.scheme() == "https";
    let local_http = url.scheme() == "http"
        && url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(|address| address.is_loopback())
        });
    if !secure && !local_http {
        return Err(AuthorizationError::InsecureJwksUrl);
    }
    Ok(url)
}

fn load_ca_certificates(path: &Path) -> Result<Vec<reqwest::Certificate>, AuthorizationError> {
    let pem = std::fs::read(path)
        .map_err(|source| AuthorizationError::ReadJwksCaCertificate { path: path.to_owned(), source })?;
    let certificates = reqwest::Certificate::from_pem_bundle(&pem)
        .map_err(|source| AuthorizationError::InvalidJwksCaCertificate { path: path.to_owned(), source })?;
    if certificates.is_empty() {
        return Err(AuthorizationError::EmptyJwksCaCertificate { path: path.to_owned() });
    }
    Ok(certificates)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_trusted_transport_urls_are_accepted() {
        for url in ["https://issuer.example/keys", "http://localhost/keys", "http://127.0.0.1/keys"] {
            assert!(parse_jwks_url(url.parse().unwrap()).is_ok(), "{url}");
        }
        for url in ["http://issuer.example/keys", "file:///keys"] {
            assert!(parse_jwks_url(url.parse().unwrap()).is_err(), "{url}");
        }
    }
}
