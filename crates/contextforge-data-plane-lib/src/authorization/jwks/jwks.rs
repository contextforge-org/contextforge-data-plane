use std::time::Duration;

use futures::StreamExt as _;
use jsonwebtoken::{
    Algorithm, AlgorithmFamily, DecodingKey, Header, Validation, decode,
    jwk::{Jwk, JwkSet, KeyOperations, PublicKeyUse},
};
use lru_time_cache::LruCache;

use reqwest::Url;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::debug;
use typed_builder::TypedBuilder;

use crate::authorization::{AuthorizationClaims, AuthorizationError};

pub const JWKS_CACHE_TTL: Duration = Duration::from_mins(5);
pub const JWKS_CACHE_KEY: &str = "jwks";

const JWKS_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(TypedBuilder)]
pub(super) struct Jwks {
    client: reqwest::Client,
    url: Url,
    issuer: String,
    audiences: Vec<String>,
    #[builder(default = RwLock::new(LruCache::with_expiry_duration(JWKS_CACHE_TTL)))]
    cache: RwLock<LruCache<String, Vec<VerificationKey>>>,
    #[builder(default = true)]
    validate_audience: bool,
    #[builder(default = true)]
    validate_expiry: bool,
    #[builder(default = true)]
    validate_not_before: bool,
}

impl Jwks {
    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&self.audiences);
        validation.validate_aud = self.validate_audience;
        validation.validate_exp = self.validate_expiry;
        validation.validate_nbf = self.validate_not_before;
        validation
    }

    pub async fn validate(&self, token: &str, header: &Header) -> Option<AuthorizationClaims> {
        {
            let cache = self.cache.read().await;

            if let Some(keys) = cache.peek(JWKS_CACHE_KEY)
                && keys.iter().any(|key| key.matches(header))
            {
                return Self::validate_with_keys(keys, token, header, &self.validation(header.alg));
            }
        }

        match fetch_jwks(&self.client, &self.url).await {
            Ok(keys) => {
                let key_count = keys.len();
                let claims = Self::validate_with_keys(&keys, token, header, &self.validation(header.alg));
                self.cache.write().await.insert(JWKS_CACHE_KEY.to_owned(), keys);
                tracing::info!("validate: SaaS JWKS cache refreshed {key_count}");

                claims
            },
            Err(error) => {
                tracing::info!("validate: unable to refresh SaaS JWKS {error:?}");
                None
            },
        }
    }

    fn validate_with_keys(
        keys: &[VerificationKey],
        token: &str,
        header: &Header,
        validation: &Validation,
    ) -> Option<AuthorizationClaims> {
        keys.iter()
            .filter(|key| key.matches(header))
            .find_map(|key| Self::validate_and_decode_claims(token, &key.decoding_key, validation))
    }

    fn validate_and_decode_claims(
        token: &str,
        key: &DecodingKey,
        validation: &Validation,
    ) -> Option<AuthorizationClaims> {
        let claims = decode::<Value>(token, key, validation)
            .inspect_err(|e| {
                debug!("validate_and_decode_claims: problem {e:?}");
            })
            .ok()?
            .claims;

        Some(AuthorizationClaims::from(claims))
    }
}

pub struct VerificationKey {
    pub(crate) key_id: Option<String>,
    pub(crate) decoding_key: DecodingKey,
}

impl VerificationKey {
    fn from_jwk(jwk: Jwk) -> Result<Option<Self>, AuthorizationError> {
        if jwk.common.public_key_use.as_ref().is_some_and(|key_use| key_use != &PublicKeyUse::Signature)
            || jwk.common.key_operations.as_ref().is_some_and(|operations| !operations.contains(&KeyOperations::Verify))
        {
            return Ok(None);
        }

        let decoding_key = DecodingKey::from_jwk(&jwk).map_err(AuthorizationError::InvalidKey)?;
        if !matches!(decoding_key.family(), AlgorithmFamily::Rsa | AlgorithmFamily::Ec) {
            return Ok(None);
        }

        Ok(Some(Self { key_id: jwk.common.key_id, decoding_key }))
    }

    pub(super) fn matches(&self, header: &Header) -> bool {
        self.decoding_key.family() == header.alg.family()
            && header
                .kid
                .as_ref()
                .is_none_or(|header_key_id| self.key_id.as_ref().is_none_or(|key_id| key_id == header_key_id))
    }
}

async fn fetch_jwks(client: &reqwest::Client, url: &Url) -> Result<Vec<VerificationKey>, AuthorizationError> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(AuthorizationError::JwksRequest)?;
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(JWKS_MAX_RESPONSE_BYTES).unwrap_or(u64::MAX))
    {
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
    if jwks.keys.is_empty() { Ok(Vec::new()) } else { validated_json_web_keys(jwks.keys) }
}

pub(super) fn validated_json_web_keys(
    jwks: impl IntoIterator<Item = Jwk>,
) -> Result<Vec<VerificationKey>, AuthorizationError> {
    let mut keys = Vec::new();
    for jwk in jwks {
        if let Some(key) = VerificationKey::from_jwk(jwk)? {
            keys.push(key);
        }
    }

    if keys.is_empty() {
        return Err(AuthorizationError::NoSupportedKeys);
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JwksConfig, get_authorization_service};
    use jsonwebtoken::{EncodingKey, encode};
    use serde_json::json;

    fn signing_key() -> EncodingKey {
        EncodingKey::from_rsa_pem(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/jwt.key"))).unwrap()
    }

    fn public_key() -> Jwk {
        let mut jwk = Jwk::from_encoding_key(&signing_key(), Algorithm::RS256).unwrap();
        jwk.common.key_id = Some("test".into());
        jwk
    }

    #[tokio::test]
    async fn verifies_signed_tokens_and_requires_trusted_claims() {
        let config = JwksConfig::default();
        let verifier = Jwks::builder()
            .client(reqwest::Client::new())
            .url(config.url)
            .issuer(config.issuer)
            .audiences(config.audiences)
            .build();
        verifier
            .cache
            .write()
            .await
            .insert(JWKS_CACHE_KEY.to_owned(), vec![VerificationKey::from_jwk(public_key()).unwrap().unwrap()]);
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test".into());
        let claims =
            json!({"iss":"mcpgateway", "aud":"mcpgateway-api", "exp":jsonwebtoken::get_current_timestamp()+3600});
        let signed = |claims: &Value, header: &Header| encode(header, claims, &signing_key()).unwrap();
        assert!(verifier.validate(&signed(&claims, &header), &header).await.is_some());
        let mut invalid = Vec::new();
        for (name, value) in
            [("iss", json!("other")), ("aud", json!("other")), ("exp", json!(1)), ("nbf", json!(9_999_999_999_u64))]
        {
            let mut modified = claims.clone();
            modified[name] = value;
            invalid.push(modified);
        }
        for name in ["iss", "aud", "exp"] {
            let mut modified = claims.clone();
            modified.as_object_mut().unwrap().remove(name);
            invalid.push(modified);
        }
        for claims in invalid {
            assert!(verifier.validate(&signed(&claims, &header), &header).await.is_none());
        }
        let mut another_rsa_algorithm = header.clone();
        another_rsa_algorithm.alg = Algorithm::RS384;
        assert!(verifier.validate(&signed(&claims, &another_rsa_algorithm), &another_rsa_algorithm).await.is_some());
    }

    #[test]
    fn requires_explicit_issuer_and_audience() {
        let config = JwksConfig::default();
        for config in [
            JwksConfig { issuer: String::new(), ..config.clone() },
            JwksConfig { audiences: vec![], ..config.clone() },
            JwksConfig { audiences: vec![String::new()], ..config },
        ] {
            assert!(get_authorization_service(&config).is_err());
        }
    }
}
