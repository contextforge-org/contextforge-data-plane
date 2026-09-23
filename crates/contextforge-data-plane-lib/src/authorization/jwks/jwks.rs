use std::time::Duration;

use futures::StreamExt as _;
use jsonwebtoken::{
    Algorithm, DecodingKey, Header, Validation, decode,
    jwk::{Jwk, JwkSet, KeyOperations, PublicKeyUse},
};
use lru_time_cache::LruCache;
use reqwest::Url;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::{
    JwksConfig,
    authorization::{AuthenticationError, AuthorizationClaims, AuthorizationError},
};

const JWKS_CACHE_TTL: Duration = Duration::from_mins(5);
const JWKS_CACHE_KEY: &str = "jwks";
const JWKS_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) struct Jwks {
    client: reqwest::Client,
    url: Url,
    validation: Validation,
    cache: RwLock<LruCache<String, Vec<VerificationKey>>>,
}

impl Jwks {
    pub fn new(client: reqwest::Client, url: Url, validation: Validation) -> Self {
        Self { client, url, validation, cache: RwLock::new(LruCache::with_expiry_duration(JWKS_CACHE_TTL)) }
    }

    pub fn validation(config: &JwksConfig) -> Result<Validation, AuthorizationError> {
        if config.issuer.trim().is_empty()
            || config.audiences.is_empty()
            || config.audiences.iter().any(|aud| aud.trim().is_empty())
        {
            return Err(AuthorizationError::InvalidTrustConfiguration);
        }
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.set_issuer(&[&config.issuer]);
        validation.set_audience(&config.audiences);
        validation.validate_nbf = true;
        validation.leeway = 30;
        Ok(validation)
    }

    pub async fn validate(&self, token: &str, header: &Header) -> Result<AuthorizationClaims, AuthenticationError> {
        if !self.validation.algorithms.contains(&header.alg)
            || header.kid.as_ref().is_none_or(|kid| kid.trim().is_empty())
        {
            return Err(AuthenticationError::InvalidToken);
        }
        {
            let cache = self.cache.read().await;
            if let Some(keys) = cache.peek(JWKS_CACHE_KEY)
                && let Some(result) = self.validate_with_keys(keys, token, header)
            {
                return result;
            }
        }
        let keys = fetch_jwks(&self.client, &self.url).await.map_err(|_| {
            tracing::warn!("jwks_refresh - unable to retrieve usable verification keys");
            AuthenticationError::KeysUnavailable
        })?;
        let claims = self.validate_with_keys(&keys, token, header).unwrap_or(Err(AuthenticationError::InvalidToken));
        self.cache.write().await.insert(JWKS_CACHE_KEY.to_owned(), keys);
        claims
    }

    fn validate_with_keys(
        &self,
        keys: &[VerificationKey],
        token: &str,
        header: &Header,
    ) -> Option<Result<AuthorizationClaims, AuthenticationError>> {
        let key = keys.iter().find(|key| key.matches(header))?;
        Some(
            decode::<Value>(token, &key.decoding_key, &self.validation)
                .map(|token| AuthorizationClaims::from(token.claims))
                .map_err(|_| AuthenticationError::InvalidToken),
        )
    }
}

struct VerificationKey {
    key_id: String,
    decoding_key: DecodingKey,
    algorithm: Option<jsonwebtoken::Algorithm>,
}

impl VerificationKey {
    fn from_jwk(jwk: &Jwk) -> Result<Option<Self>, AuthorizationError> {
        if jwk.common.public_key_use.as_ref().is_some_and(|key_use| key_use != &PublicKeyUse::Signature)
            || jwk.common.key_operations.as_ref().is_some_and(|operations| !operations.contains(&KeyOperations::Verify))
        {
            return Ok(None);
        }
        let Some(key_id) = jwk.common.key_id.as_ref().filter(|kid| !kid.trim().is_empty()) else {
            return Ok(None);
        };
        // Ignore symmetric and unsupported key types before decoding.
        if !matches!(
            jwk.algorithm,
            jsonwebtoken::jwk::AlgorithmParameters::RSA(_) | jsonwebtoken::jwk::AlgorithmParameters::EllipticCurve(_)
        ) {
            return Ok(None);
        }
        let algorithm = match jwk.common.key_algorithm {
            Some(alg) => match jsonwebtoken::Algorithm::try_from(alg) {
                Ok(alg) => Some(alg),
                Err(_) => return Ok(None),
            },
            None => None,
        };
        let decoding_key = DecodingKey::from_jwk(jwk).map_err(AuthorizationError::InvalidKey)?;
        Ok(Some(Self { key_id: key_id.clone(), decoding_key, algorithm }))
    }

    fn matches(&self, header: &Header) -> bool {
        header.kid.as_ref() == Some(&self.key_id)
            && self.decoding_key.family() == header.alg.family()
            && self.algorithm.is_none_or(|alg| alg == header.alg)
    }
}

async fn fetch_jwks(client: &reqwest::Client, url: &Url) -> Result<Vec<VerificationKey>, AuthorizationError> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(AuthorizationError::JwksRequest)?;
    if response.content_length().is_some_and(|length| length > JWKS_MAX_RESPONSE_BYTES as u64) {
        return Err(AuthorizationError::JwksResponseTooLarge);
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(AuthorizationError::JwksRequest)?;
        if chunk.len() > JWKS_MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err(AuthorizationError::JwksResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    let jwks = serde_json::from_slice::<JwkSet>(&body).map_err(AuthorizationError::InvalidJson)?;
    let mut keys: Vec<VerificationKey> = Vec::new();
    for jwk in jwks.keys {
        if let Some(key) = VerificationKey::from_jwk(&jwk)? {
            if keys.iter().any(|existing| existing.key_id == key.key_id) {
                return Err(AuthorizationError::DuplicateKeyId);
            }
            keys.push(key);
        }
    }
    if keys.is_empty() {
        return Err(AuthorizationError::NoSupportedKeys);
    }
    Ok(keys)
}

#[cfg(test)]
mod tests;
