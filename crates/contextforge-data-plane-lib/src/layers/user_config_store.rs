use std::collections::HashSet;

use axum::{extract::State, middleware::Next, response::Response};
use contextforge_data_plane_apis::User;
use cpex::cpex_core::extensions::{SubjectExtension, SubjectType};
use serde_json::Value;

use tracing::{debug, info, warn};

use crate::{
    authorization::AuthorizedPrincipal,
    common::ContextForgeDataPlaneAppState,
    errors::{bad_request, internal_server_error},
    user_config_store::ConfigStoreError,
};

pub async fn user_config_store_layer(
    State(state): State<ContextForgeDataPlaneAppState>,
    mut request: http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let maybe_principal = request.extensions().get::<AuthorizedPrincipal>();
    if let Some(principal) = maybe_principal {
        debug!(
            "user_config_store_layer - getting user config for principal {principal:?} method = {method} path = {path}"
        );
        let user = User::from(principal);
        let config = state.config_store.get_config(&user).await;
        if let Some(plugin_request) = request.extensions().get::<contextforge_data_plane_cpex::PluginRequest>()
            && let Some(claims) = request.extensions().get::<crate::AuthorizationClaims>()
        {
            let email = config.as_ref().ok().and_then(|config| config.user_email.as_deref());
            plugin_request.set_verified_subject(verified_subject(principal, claims, email)).await;
        }
        match config {
            Ok(user_config) => {
                let virtual_hosts = user_config.virtual_hosts.len();
                info!(
                    "user_config_store_layer - loaded user config principal = {principal:?} virtual_hosts = {virtual_hosts}"
                );
                request.extensions_mut().insert(user_config);
                next.run(request).await
            },

            Err(ConfigStoreError::NoDataForKey) => {
                debug!(
                    "user_config_store_layer - user config lookup returned no data principal = {principal:?} method = {method} path = {path}"
                );
                bad_request("Problem occurred retrieving the configuration")
            },

            Err(error) => {
                debug!(
                    "user_config_store_layer - user config lookup failed principal = {principal:?} method = {method} path = {path} error = {error}"
                );
                internal_server_error("Problem occurred retrieving the configuration")
            },
        }
    } else {
        warn!("user_config_store_layer - no claims found in request extensions method = {method} path = {path}");
        bad_request("No claims in the token")
    }
}

fn verified_subject(
    principal: &crate::authorization::AuthorizedPrincipal,
    claims: &crate::AuthorizationClaims,
    email: Option<&str>,
) -> SubjectExtension {
    let claims = Value::from(claims);
    let user = User::from(principal);
    SubjectExtension {
        id: Some(email.unwrap_or(user.key()).to_owned()),
        subject_type: Some(SubjectType::User),
        roles: string_set(claims.get("roles")),
        teams: string_set(claims.get("teams")),
        permissions: string_set(claims.get("scopes").and_then(|scopes| scopes.get("permissions"))),
        claims: claims
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), value.as_str().map_or_else(|| value.to_string(), str::to_owned)))
            .collect(),
    }
}

fn string_set(value: Option<&Value>) -> HashSet<String> {
    value.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}
