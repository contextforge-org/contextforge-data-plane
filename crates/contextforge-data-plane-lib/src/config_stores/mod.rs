mod global_config_store;
mod redis_store;
mod user_config_store;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;

use lru_time_cache::LruCache;
use serde::{Deserialize, Serialize};

pub use redis_store::RedisStore;
use tokio::sync::Mutex;

type ConfigCache<V> = Option<Arc<Mutex<LruCache<String, CachedEntry<V>>>>>;

#[derive(Debug, Clone, Deserialize, Serialize, thiserror::Error)]
pub enum ConfigStoreError {
    #[error("data store disconnected")]
    InvalidConnection,
    #[error("no data for key")]
    NoDataForKey,
    #[error("data in wrong format")]
    DataWrongFormat,
    #[error("unable to encode the data")]
    DataEncoding,
    #[error("unable to write to store")]
    CantWriteData,
}

#[derive(Clone)]
struct CachedEntry<V> {
    inserted_at: Instant,
    config: V,
}

impl<V> CachedEntry<V> {
    fn new(config: V) -> Self {
        Self { inserted_at: Instant::now(), config }
    }

    fn is_fresh(&self, expiry: Duration) -> bool {
        self.inserted_at.elapsed() < expiry
    }
}

pub trait Key {
    fn key(&self) -> &str;
}

#[async_trait]
pub trait ConfigStore<K: Key, V>: Send + Sync {
    async fn get_config<'a>(&self, key: &'a K) -> Result<V, ConfigStoreError>;
    async fn set_config<'a>(&self, key: &'a K, config: &'a V) -> Result<(), ConfigStoreError>;
}
