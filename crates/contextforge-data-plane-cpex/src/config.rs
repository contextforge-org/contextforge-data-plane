use async_trait::async_trait;
use redis::{
    Client, RedisError,
    aio::{ConnectionManager, ConnectionManagerConfig},
    cmd,
};
use tokio::sync::Mutex;

use crate::error::GatewayPluginRuntimeError;
use contextforge_data_plane_apis::runtime_plugin_config::{RUNTIME_PLUGIN_CONFIG_KEY, RuntimePluginConfigDocument};

#[async_trait]
pub(crate) trait RuntimePluginConfigStore: Send + Sync {
    async fn get_config(&self) -> Result<Option<LoadedRuntimePluginConfig>, GatewayPluginRuntimeError>;
}

#[derive(Clone, Debug)]
pub(crate) struct LoadedRuntimePluginConfig {
    pub(crate) document: RuntimePluginConfigDocument,
    pub(crate) fingerprint: Vec<u8>,
}

impl LoadedRuntimePluginConfig {
    pub(crate) fn decode(config: Vec<u8>) -> Result<Self, GatewayPluginRuntimeError> {
        let document = decode_config_document(&config)?;
        Ok(Self { document, fingerprint: config })
    }
}

pub(crate) struct RedisRuntimePluginConfigStore {
    redis_client: Client,
    connection: Mutex<Option<ConnectionManager>>,
}

impl RedisRuntimePluginConfigStore {
    pub(crate) fn new(redis_client: Client) -> Self {
        Self { redis_client, connection: Mutex::new(None) }
    }

    async fn connection(&self) -> Result<ConnectionManager, GatewayPluginRuntimeError> {
        let mut connection = self.connection.lock().await;
        if connection.is_none() {
            *connection = Some(
                self.redis_client
                    .get_connection_manager_with_config(ConnectionManagerConfig::default())
                    .await
                    .map_err(|_| GatewayPluginRuntimeError::ConfigStoreUnavailable)?,
            );
        }
        connection.clone().ok_or(GatewayPluginRuntimeError::ConfigStoreUnavailable)
    }
}

#[async_trait]
impl RuntimePluginConfigStore for RedisRuntimePluginConfigStore {
    async fn get_config(&self) -> Result<Option<LoadedRuntimePluginConfig>, GatewayPluginRuntimeError> {
        let mut connection = self.connection().await?;

        let maybe_config: Result<Option<Vec<u8>>, RedisError> =
            cmd("GET").arg(RUNTIME_PLUGIN_CONFIG_KEY).take().query_async(&mut connection).await;
        let Some(config) = maybe_config.map_err(|_| GatewayPluginRuntimeError::ConfigStoreUnavailable)? else {
            return Ok(None);
        };

        LoadedRuntimePluginConfig::decode(config).map(Some)
    }
}

pub(crate) fn decode_config_document(config: &[u8]) -> Result<RuntimePluginConfigDocument, GatewayPluginRuntimeError> {
    serde_json::from_slice::<RuntimePluginConfigDocument>(config)
        .or_else(|_| rmp_serde::decode::from_slice::<RuntimePluginConfigDocument>(config))
        .map_err(|_| GatewayPluginRuntimeError::ConfigWrongFormat)
}

#[cfg(test)]
mod tests {
    use super::decode_config_document;

    #[test]
    fn decode_config_document_accepts_json_bytes() {
        let document = br#" { "enabled": true, "global": { "plugins": null }, "contexts": {} }"#;

        let document = decode_config_document(document).expect("JSON document decodes");

        assert!(document.global.expect("global config exists").plugins.is_empty());
    }

    #[test]
    fn decode_config_document_accepts_messagepack_bytes() {
        let expected = serde_json::json!({"enabled": true, "global": {"plugins": []}, "contexts": {}});
        let document = rmp_serde::to_vec_named(&expected).expect("MessagePack document encodes");

        assert!(decode_config_document(&document).expect("MessagePack document decodes").enabled);
    }

    #[test]
    fn decode_config_document_rejects_missing_configuration() {
        let error = decode_config_document(br#"{ "enabled": true }"#).expect_err("missing config is rejected");

        assert_eq!("runtime plugin config is in wrong format", error.to_string());
    }

    #[test]
    fn decode_config_document_rejects_invalid_json_bytes() {
        let error = decode_config_document(b"{not-json").expect_err("invalid JSON bytes are rejected");

        assert_eq!("runtime plugin config is in wrong format", error.to_string());
    }
}
