use async_trait::async_trait;
use contextforge_data_plane_apis::GlobalConfig;

use super::ConfigStoreError;
use crate::config_stores::{ConfigStore, Key};

impl Key for GlobalConfig {
    fn key(&self) -> &'static str {
        "CONTEXT_FORGE_GLOBAL_CONFIG"
    }
}

#[async_trait]
impl ConfigStore<GlobalConfig, GlobalConfig> for super::RedisStore<GlobalConfig> {
    async fn get_config<'a>(&self, key: &'a GlobalConfig) -> Result<GlobalConfig, ConfigStoreError> {
        self.get_config(key).await
    }
    async fn set_config<'a>(&self, key: &'a GlobalConfig, config: &'a GlobalConfig) -> Result<(), ConfigStoreError> {
        self.set_config(key, config).await
    }
}
