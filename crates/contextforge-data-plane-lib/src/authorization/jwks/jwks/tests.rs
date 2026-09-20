use super::*;
use axum::{Router, routing::get};
use http::StatusCode;
use jsonwebtoken::{Algorithm, EncodingKey, encode};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    verifier: Jwks,
    body: Arc<RwLock<(StatusCode, Vec<u8>)>>,
    requests: Arc<AtomicUsize>,
    delay_ms: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(document: Value) -> Self {
        let body = Arc::new(RwLock::new((StatusCode::OK, serde_json::to_vec(&document).unwrap())));
        let response = Arc::clone(&body);
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        let delay_ms = Arc::new(AtomicUsize::new(0));
        let delay = Arc::clone(&delay_ms);
        let app = Router::new().route(
            "/",
            get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                let response = Arc::clone(&response);
                let delay = delay.load(Ordering::SeqCst);
                async move {
                    tokio::time::sleep(Duration::from_millis(delay as u64)).await;
                    response.read().await.clone()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap()).parse().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let verifier = Jwks::new(reqwest::Client::new(), url, Jwks::validation(&JwksConfig::default()).unwrap());
        Self { verifier, body, requests, delay_ms, task }
    }
    async fn allow_refresh(&self) {
        self.verifier.refresh.lock().await.last_attempt = None;
    }
    async fn expire(&self) {
        self.verifier.cache.write().await.as_mut().unwrap().expires_at = Instant::now();
        self.allow_refresh().await;
    }
    async fn verify(&self, kid: &str) -> Result<AuthorizationClaims, AuthenticationError> {
        let (token, header) = token(kid);
        self.verifier.validate(&token, &header).await
    }
}
fn key() -> EncodingKey {
    EncodingKey::from_rsa_pem(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/jwt.key"))).unwrap()
}
fn document(kid: &str) -> Value {
    let mut key = Jwk::from_encoding_key(&key(), Algorithm::RS256).unwrap();
    key.common.key_id = Some(kid.into());
    json!({"keys":[key]})
}
fn token(kid: &str) -> (String, Header) {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.into());
    let claims = json!({"iss":"mcpgateway","aud":"mcpgateway-api","exp":jsonwebtoken::get_current_timestamp()+3600});
    (encode(&header, &claims, &key()).unwrap(), header)
}

#[tokio::test]
async fn concurrent_cold_requests_and_unknown_keys_have_bounded_refreshes() {
    let fixture = Fixture::new(document("one")).await;
    let results = futures::future::join_all((0..32).map(|_| fixture.verify("one"))).await;
    assert!(results.into_iter().all(|result| result.is_ok()));
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    fixture.allow_refresh().await;
    let results = futures::future::join_all((0..32).map(|_| fixture.verify("unknown"))).await;
    assert!(results.into_iter().all(|result| result == Err(AuthenticationError::InvalidToken)));
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    assert!(fixture.verify("one").await.is_ok());
}

#[tokio::test]
async fn rotation_replaces_keys_and_expired_keys_fail_closed_during_outage() {
    let fixture = Fixture::new(document("one")).await;
    assert!(fixture.verify("one").await.is_ok());
    *fixture.body.write().await = (StatusCode::OK, serde_json::to_vec(&document("two")).unwrap());
    fixture.allow_refresh().await;
    assert!(fixture.verify("two").await.is_ok());
    assert_eq!(fixture.verify("one").await, Err(AuthenticationError::InvalidToken));
    *fixture.body.write().await = (StatusCode::SERVICE_UNAVAILABLE, Vec::new());
    fixture.allow_refresh().await;
    assert_eq!(fixture.verify("unknown").await, Err(AuthenticationError::KeysUnavailable));
    // Known, unexpired keys remain usable during a failed refresh for a different kid.
    assert!(fixture.verify("two").await.is_ok());
    fixture.expire().await;
    assert_eq!(fixture.verify("two").await, Err(AuthenticationError::KeysUnavailable));
    *fixture.body.write().await = (StatusCode::OK, serde_json::to_vec(&document("two")).unwrap());
    fixture.allow_refresh().await;
    assert!(fixture.verify("two").await.is_ok());
}

#[tokio::test]
async fn rejects_unusable_key_sets_and_declared_algorithm_mismatch() {
    let document = document("one");
    let mut duplicate = document.clone();
    duplicate["keys"].as_array_mut().unwrap().push(document["keys"][0].clone());
    let mut encryption_key = document.clone();
    encryption_key["keys"][0]["use"] = "enc".into();
    let mut non_verify = document.clone();
    non_verify["keys"][0]["key_ops"] = json!(["sign"]);
    let mut no_kid = document.clone();
    no_kid["keys"][0].as_object_mut().unwrap().remove("kid");
    for document in [json!({"keys":[]}), duplicate, encryption_key, non_verify, no_kid] {
        let fixture = Fixture::new(document).await;
        assert_eq!(fixture.verify("one").await, Err(AuthenticationError::KeysUnavailable));
    }
    let mut mismatch = document.clone();
    mismatch["keys"][0]["alg"] = "RS384".into();
    let fixture = Fixture::new(mismatch).await;
    assert_eq!(fixture.verify("one").await, Err(AuthenticationError::InvalidToken));
}

#[tokio::test]
async fn bounded_response_and_malformed_json_are_unavailable() {
    for body in [b"not-json".to_vec(), vec![b' '; JWKS_MAX_RESPONSE_BYTES + 1]] {
        let fixture = Fixture::new(document("one")).await;
        *fixture.body.write().await = (StatusCode::OK, body);
        assert_eq!(fixture.verify("one").await, Err(AuthenticationError::KeysUnavailable));
        assert_eq!(fixture.verify("one").await, Err(AuthenticationError::KeysUnavailable));
        assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn rejects_incomplete_or_unsafe_trust_configuration() {
    let config = JwksConfig::default();
    for config in [
        JwksConfig { issuer: String::new(), ..config.clone() },
        JwksConfig { audiences: vec![], ..config.clone() },
        JwksConfig { audiences: vec![String::new()], ..config.clone() },
        JwksConfig { algorithms: vec![], ..config.clone() },
        JwksConfig { algorithms: vec![Algorithm::HS256], ..config.clone() },
        JwksConfig { leeway_seconds: 301, ..config },
    ] {
        assert!(Jwks::validation(&config).is_err());
    }
}

#[tokio::test]
async fn cancelled_fetch_cannot_bypass_refresh_cooldown() {
    let fixture = Fixture::new(document("one")).await;
    fixture.delay_ms.store(5000, Ordering::SeqCst);
    assert!(tokio::time::timeout(Duration::from_millis(100), fixture.verify("one")).await.is_err());
    assert_eq!(fixture.verify("one").await, Err(AuthenticationError::KeysUnavailable));
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
}
