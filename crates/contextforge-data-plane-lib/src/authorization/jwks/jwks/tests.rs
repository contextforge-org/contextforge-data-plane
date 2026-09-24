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
    let claims = json!({"iss":"mcpgateway", "aud":"mcpgateway-api", "exp":jsonwebtoken::get_current_timestamp()+3600});
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
    let mut wrong_algorithm = header.clone();
    wrong_algorithm.alg = Algorithm::RS384;
    assert!(verifier.validate(&signed(&claims, &wrong_algorithm), &wrong_algorithm).await.is_none());
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
