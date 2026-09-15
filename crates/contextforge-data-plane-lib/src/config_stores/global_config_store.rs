use std::time::Duration;

use async_trait::async_trait;
use contextforge_data_plane_apis::GlobalConfig;
use serde::Serialize;

use super::ConfigStoreError;
use crate::{
    RedisClient, RedisConfig,
    config_stores::{ConfigStore, Key, RedisStore},
};

#[derive(Serialize)]
struct GlobalConfigKey;

impl Key for GlobalConfigKey {
    fn key(&self) -> &'static str {
        "CONTEXT_FORGE_GLOBAL_CONFIG"
    }
}

#[async_trait]
impl ConfigStore<GlobalConfigKey, GlobalConfig> for super::RedisStore<GlobalConfig> {
    async fn get_config<'a>(&self, key: &'a GlobalConfigKey) -> Result<GlobalConfig, ConfigStoreError> {
        self.get_config(key).await
    }
    async fn set_config<'a>(&self, key: &'a GlobalConfigKey, config: &'a GlobalConfig) -> Result<(), ConfigStoreError> {
        self.set_config(key, config).await
    }
}

pub async fn get_global_config(config: &RedisConfig) -> Result<GlobalConfig, ConfigStoreError> {
    let store = RedisStore::new(
        &RedisClient::try_from(config.clone()).map_err(|e| ConfigStoreError::InvalidConfiguration(e.to_string()))?,
        Duration::from_secs(5),
    )
    .await
    .map_err(|e| ConfigStoreError::InvalidConfiguration(e.to_string()))?;

    store.get_config(&GlobalConfigKey).await
}

#[cfg(feature = "with_tools")]
pub async fn set_global_config(config: &RedisConfig, global_config: GlobalConfig) -> Result<(), ConfigStoreError> {
    let store = RedisStore::new(
        &RedisClient::try_from(config.clone()).map_err(|e| ConfigStoreError::InvalidConfiguration(e.to_string()))?,
        Duration::from_secs(5),
    )
    .await
    .map_err(|e| ConfigStoreError::InvalidConfiguration(e.to_string()))?;

    store.set_config(&GlobalConfigKey, &global_config).await
}
