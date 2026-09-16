use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use contextforge_data_plane_apis::{User, user_store::UserConfig};
use contextforge_data_plane_lib::{ConfigStore, ConfigStoreError};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub(crate) struct MemoryUserConfigStore {
    configs: Arc<Mutex<HashMap<String, UserConfig>>>,
}

#[async_trait]
impl ConfigStore<User, UserConfig> for MemoryUserConfigStore {
    async fn get_config<'a>(&self, key: &'a User) -> Result<UserConfig, ConfigStoreError> {
        self.configs.lock().await.get(key.key()).cloned().ok_or(ConfigStoreError::NoDataForKey)
    }

    async fn set_config<'a>(&self, key: &'a User, config: &'a UserConfig) -> Result<(), ConfigStoreError> {
        self.configs.lock().await.insert(key.key().to_owned(), config.clone());
        Ok(())
    }
}
