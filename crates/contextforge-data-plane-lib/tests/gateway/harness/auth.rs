use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use contextforge_data_plane_lib::{AuthorizationClaims, AuthorizationService};
use http::HeaderValue;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::json;

use super::TEST_USER_EMAIL;

const TEST_TOKEN_TTL_SECS: u64 = 60 * 60;

fn default_claims(user_id: &str) -> serde_json::Value {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock").as_secs();
    json!({
        "iss": "mcpgateway",
        "sub": user_id,
        "tenant_id": "test_tenant",
        "aud": "mcpgateway-api",
        "exp": now + TEST_TOKEN_TTL_SECS,
        "iat": now,
        "jti": "test-token",
        "token_use": "api",
        "teams": ["team_awesome"],
        "user": {
            "email": TEST_USER_EMAIL,
            "full_name": "API Token User",
            "is_admin": true,
            "auth_provider": "api_token"
        },
        "scopes": {
            "server_id": "my_id",
            "permissions": ["tools.read", "servers.use"],
            "ip_restrictions": ["192.169.1.0/24"],
            "time_restrictions": null
        },
    })
}

pub(crate) fn token(user_id: &str) -> String {
    let key = EncodingKey::from_rsa_pem(&fs::read("../../assets/jwt.key").expect("jwt key")).expect("encoding key");
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test".to_owned());
    encode(&header, &default_claims(user_id), &key).expect("jwt token")
}

#[derive(Debug)]
pub struct AlwaysAllowAuthorizatioService {
    user: String,
}

impl AlwaysAllowAuthorizatioService {
    pub fn new(user: String) -> AlwaysAllowAuthorizatioService {
        Self { user }
    }
}

#[async_trait]
impl AuthorizationService for AlwaysAllowAuthorizatioService {
    async fn authorize(&self, _: &HeaderValue) -> Option<AuthorizationClaims> {
        Some(AuthorizationClaims::from(default_claims(&self.user)))
    }
}
