use super::*;
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
    let verifier = Jwks::new(reqwest::Client::new(), config.url.clone(), Jwks::validation(&config).unwrap());
    verifier
        .cache
        .write()
        .await
        .insert(JWKS_CACHE_KEY.to_owned(), vec![VerificationKey::from_jwk(&public_key()).unwrap().unwrap()]);
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test".into());
    let claims = json!({"iss":"mcpgateway", "aud":"mcpgateway-api", "exp":jsonwebtoken::get_current_timestamp()+3600});
    let signed = |claims: &Value, header: &Header| encode(header, claims, &signing_key()).unwrap();
    assert!(verifier.validate(&signed(&claims, &header), &header).await.is_ok());
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
        assert_eq!(verifier.validate(&signed(&claims, &header), &header).await, Err(AuthenticationError::InvalidToken));
    }
    let mut wrong_algorithm = header.clone();
    wrong_algorithm.alg = Algorithm::RS384;
    assert_eq!(
        verifier.validate(&signed(&claims, &wrong_algorithm), &wrong_algorithm).await,
        Err(AuthenticationError::InvalidToken)
    );
    header.kid = None;
    assert_eq!(verifier.validate(&signed(&claims, &header), &header).await, Err(AuthenticationError::InvalidToken));
}

#[test]
fn selects_only_the_matching_signing_key() {
    let mut jwk = public_key();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test".into());
    assert!(VerificationKey::from_jwk(&jwk).unwrap().unwrap().matches(&header));
    header.kid = Some("other".into());
    assert!(!VerificationKey::from_jwk(&jwk).unwrap().unwrap().matches(&header));
    header.kid = Some("test".into());
    jwk.common.key_algorithm = Some(jsonwebtoken::jwk::KeyAlgorithm::RS384);
    assert!(!VerificationKey::from_jwk(&jwk).unwrap().unwrap().matches(&header));
    jwk.common.public_key_use = Some(PublicKeyUse::Encryption);
    assert!(VerificationKey::from_jwk(&jwk).unwrap().is_none());
    jwk = public_key();
    jwk.common.key_id = None;
    assert!(VerificationKey::from_jwk(&jwk).unwrap().is_none());
}

#[test]
fn requires_explicit_issuer_and_audience() {
    let config = JwksConfig::default();
    for config in [
        JwksConfig { issuer: String::new(), ..config.clone() },
        JwksConfig { audiences: vec![], ..config.clone() },
        JwksConfig { audiences: vec![String::new()], ..config },
    ] {
        assert!(Jwks::validation(&config).is_err());
    }
}
