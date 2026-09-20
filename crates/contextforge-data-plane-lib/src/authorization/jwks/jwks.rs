use std::time::Duration;

use futures::StreamExt as _;
use jsonwebtoken::{
    AlgorithmFamily, DecodingKey, Header, Validation, decode,
    jwk::{Jwk, JwkSet, KeyOperations, PublicKeyUse},
};
use reqwest::Url;
use serde_json::Value;
use tokio::{
    sync::{Mutex, RwLock},
    time::Instant,
};

use crate::{
    JwksConfig,
    authorization::{AuthenticationError, AuthorizationClaims, AuthorizationError},
};

const JWKS_CACHE_TTL: Duration = Duration::from_mins(5);
const JWKS_REFRESH_COOLDOWN: Duration = Duration::from_secs(5);
const JWKS_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

struct CachedKeys {
    keys: Vec<VerificationKey>,
    expires_at: Instant,
}

#[derive(Default)]
struct RefreshState {
    last_attempt: Option<Instant>,
    failed: bool,
}

pub(super) struct Jwks {
    client: reqwest::Client,
    url: Url,
    validation: Validation,
    cache: RwLock<Option<CachedKeys>>,
    // One refresh in flight, including concurrent unknown-kid requests.
    refresh: Mutex<RefreshState>,
}

impl Jwks {
    pub fn new(client: reqwest::Client, url: Url, validation: Validation) -> Self {
        Self { client, url, validation, cache: RwLock::new(None), refresh: Mutex::new(RefreshState::default()) }
    }

    pub fn validation(config: &JwksConfig) -> Result<Validation, AuthorizationError> {
        if config.issuer.trim().is_empty()
            || config.audiences.is_empty()
            || config.audiences.iter().any(|aud| aud.trim().is_empty())
            || config.algorithms.is_empty()
            || config.algorithms.iter().any(|alg| !matches!(alg.family(), AlgorithmFamily::Rsa | AlgorithmFamily::Ec))
            || config.leeway_seconds > 300
        {
            return Err(AuthorizationError::InvalidTrustConfiguration);
        }
        let mut validation = Validation::new(config.algorithms[0]);
        validation.algorithms.clone_from(&config.algorithms);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.set_issuer(&[&config.issuer]);
        validation.set_audience(&config.audiences);
        validation.validate_nbf = true;
        validation.leeway = config.leeway_seconds;
        Ok(validation)
    }

    pub async fn validate(&self, token: &str, header: &Header) -> Result<AuthorizationClaims, AuthenticationError> {
        if !self.validation.algorithms.contains(&header.alg)
            || header.kid.as_ref().is_none_or(|kid| kid.trim().is_empty())
        {
            return Err(AuthenticationError::InvalidToken);
        }
        if let Some(result) = self.validate_cached(token, header).await {
            return result;
        }

        let mut refresh = self.refresh.lock().await;
        // Another request may have loaded or rotated the keys while we waited.
        if let Some(result) = self.validate_cached(token, header).await {
            return result;
        }
        if refresh.last_attempt.is_some_and(|time| time.elapsed() < JWKS_REFRESH_COOLDOWN) {
            return Err(if refresh.failed {
                AuthenticationError::KeysUnavailable
            } else {
                AuthenticationError::InvalidToken
            });
        }
        // Record before awaiting I/O so cancellation cannot bypass the cooldown.
        refresh.last_attempt = Some(Instant::now());
        refresh.failed = true;
        let result = fetch_jwks(&self.client, &self.url).await;
        refresh.last_attempt = Some(Instant::now());
        refresh.failed = result.is_err();
        let keys = result.map_err(|_| {
            // Do not log response bodies, key material, tokens, or URLs with query credentials.
            tracing::warn!("jwks_refresh - unable to retrieve usable verification keys");
            AuthenticationError::KeysUnavailable
        })?;
        let claims = self.validate_with_keys(&keys, token, header).unwrap_or(Err(AuthenticationError::InvalidToken));
        *self.cache.write().await = Some(CachedKeys { keys, expires_at: Instant::now() + JWKS_CACHE_TTL });
        tracing::info!("jwks_refresh - verification keys refreshed");
        claims
    }

    async fn validate_cached(
        &self,
        token: &str,
        header: &Header,
    ) -> Option<Result<AuthorizationClaims, AuthenticationError>> {
        let cache = self.cache.read().await;
        let cache = cache.as_ref().filter(|cache| cache.expires_at > Instant::now())?;
        self.validate_with_keys(&cache.keys, token, header)
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
