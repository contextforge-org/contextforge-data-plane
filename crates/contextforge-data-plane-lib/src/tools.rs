use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{Router, get, post},
};
use contextforge_data_plane_apis::{User as CFUser, user_store::UserConfig};
use http::{
    StatusCode,
    header::{self, CACHE_CONTROL},
};
use jsonwebtoken::jwk::{Jwk, JwkSet};
use serde::Deserialize;
use serde_json::json;
use std::{fs, time::Duration};
use uuid::Uuid;

use crate::{Scopes, authorization::AuthorizationClaims, common::ContextForgeDataPlaneAppState};

const DEFAULT_TOKEN_EMAIL: &str = "admin@example.com";
const JWKS_CACHE_CONTROL: &str = "public, max-age=300, must-revalidate";
const TOKEN_PATH: &str = "/admin/tokens/{tenant_id}/{user_id}";
const JWKS_PATH: &str = "/admin/.well-known/jwks.json";
const CONFIGURE_USER_PATH: &str = "/admin/userconfigs/{user_id}";

#[derive(Debug, Deserialize)]
pub struct TokenQuery {
    email: Option<String>,
}

async fn get_jwks(State(state): State<ContextForgeDataPlaneAppState>) -> Response {
    let Ok(key) = jsonwebtoken::EncodingKey::from_rsa_pem(
        &fs::read(&state.config.token_verification_private_key).expect("Expecting this to work"),
    ) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Can't find the encoding key or the format is wrong")
            .into_response();
    };

    let Ok(key) = Jwk::from_encoding_key(&key, jsonwebtoken::Algorithm::RS256) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Can't find the encoding key or the format is wrong")
            .into_response();
    };

    let keys = vec![key];
    (StatusCode::OK, [(CACHE_CONTROL, JWKS_CACHE_CONTROL)], Json(JwkSet { keys })).into_response()
}

pub fn add_tools(router: Router<ContextForgeDataPlaneAppState>) -> Router<ContextForgeDataPlaneAppState> {
    router
        .route(TOKEN_PATH, get(get_token))
        .route(TOKEN_PATH, post(get_custom_token))
        .route(JWKS_PATH, get(get_jwks))
        .route(CONFIGURE_USER_PATH, post(configure_user))
        .route("/health", get(health))
}

pub async fn health() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{\"status\": \"healthy\"}"))
        .expect("Expecting this to work")
}

pub async fn get_custom_token(
    State(state): State<ContextForgeDataPlaneAppState>,
    Json(mut claims): Json<serde_json::Value>,
) -> Response {
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(
        &fs::read(&state.config.token_verification_private_key).expect("Expecting this to work"),
    )
    .expect("Expecting this to work");

    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("Time went backwards").as_secs();

    claims["exp"] = (now + Duration::from_hours(1).as_secs()).into();
    claims["nbf"] = (now - Duration::from_mins(1).as_secs()).into();
    claims["iat"] = (now).into();
    claims["jti"] = Uuid::new_v4().to_string().into();

    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test".to_owned());
    let token = jsonwebtoken::encode::<serde_json::Value>(&header, &claims, &key).expect("Expecting this to work");

    token.into_response()
}

pub async fn get_token(
    State(state): State<ContextForgeDataPlaneAppState>,
    Path((tenant_id, user_id)): Path<(String, String)>,
    Query(query): Query<TokenQuery>,
) -> Response {
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(
        &fs::read(&state.config.token_verification_private_key).expect("Expecting this to work"),
    )
    .expect("Expecting this to work");

    let user_email = query.email.as_deref().unwrap_or(DEFAULT_TOKEN_EMAIL);
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("Time went backwards").as_secs();

    let map = json!( {
        "iss": "contexforge-dataplane",
        "sub": user_id.clone(),
        "aud": "contexforge-dataplane-audience",
        "exp": now + Duration::from_hours(1).as_secs(),
        "nbf": now - Duration::from_mins(1).as_secs(),
        "iat": now,
        "jti": Uuid::new_v4().to_string(),
        "token_use": Some("api".to_owned()),
        "teams": vec!["team_awesome".to_owned()],
        "user": crate::authorization::User::builder()
            .tenant_id("team_awesome".to_owned())
            .user_id(user_id.clone())
            .build(),
        "scopes": Scopes::builder()
            .server_id(Some("my_id".to_owned()))
            .ip_restrictions(vec!["192.169.1.0/24".to_owned()])
            .permissions(vec!["tools.read".to_owned(), "servers.use".to_owned()])
            .time_restrictions(None)
            .build(),
        "tenant_id": tenant_id,
        "user_email": user_email
    });

    let claims = serde_json::Value::from(AuthorizationClaims::from(map));
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test".to_owned());
    let token = jsonwebtoken::encode::<serde_json::Value>(&header, &claims, &key).expect("Expecting this to work");

    token.into_response()
}

//#[debug_handler]
pub async fn configure_user(
    Path(user_id): Path<String>,
    State(state): State<ContextForgeDataPlaneAppState>,
    Json(user_config): Json<UserConfig>,
) -> Response {
    if state.config_store.set_config(&CFUser::new(&user_id), &user_config).await.is_ok() {
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from("Added"))
            .expect("Expecting this to work")
    } else {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from("Problem with encoding "))
            .expect("Expecting this to work")
    }
}
