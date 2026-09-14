use std::{sync::Arc, time::Duration};

use lru_time_cache::LruCache;
use redis::{
    AsyncCommands, RedisError,
    aio::{ConnectionManager, ConnectionManagerConfig},
    cmd,
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::ConfigStoreError;
use crate::{
    common::RedisClient,
    config_stores::{CachedEntry, ConfigCache, Key},
    const_values::{LRU_CACHE_ENTRIES, REDIS_RETRIES},
};

#[derive(Clone)]
pub struct RedisStore<V> {
    connection: ConnectionManager,
    /// `None` when caching is disabled (zero expiry): every request reads Redis.
    cache: ConfigCache<V>,
    cache_expiry: Duration,
}

impl<V> RedisStore<V>
where
    V: Clone + Serialize + DeserializeOwned,
{
    pub async fn new(redis_client: &RedisClient, cache_expiry: Duration) -> crate::Result<Self> {
        Ok(Self {
            connection: redis_client
                .get_connection_manager_with_config(
                    ConnectionManagerConfig::default().set_number_of_retries(REDIS_RETRIES),
                )
                .await
                .map_err(|error| {
                    warn!("RedisUserConfigStore::new - failed to create Redis user config connection error = {error}");
                    ConfigStoreError::InvalidConnection
                })?,
            cache: (!cache_expiry.is_zero()).then(|| {
                Arc::new(Mutex::new(LruCache::with_expiry_duration_and_capacity(cache_expiry, LRU_CACHE_ENTRIES)))
            }),
            cache_expiry,
        })
    }

    pub async fn get_config<K>(&self, key: &K) -> Result<V, ConfigStoreError>
    where
        K: Key + Serialize,
    {
        let subject = key.key();

        if let Some(cache) = &self.cache {
            if let Some(entry) = cache.lock().await.get_mut(subject) {
                if entry.is_fresh(self.cache_expiry) {
                    debug!("RedisUserConfigStore::get_config - user config cache hit subject = {subject}");
                    return Ok(entry.config.clone());
                }

                debug!("RedisUserConfigStore::get_config - user config cache entry expired subject = {subject}");
            } else {
                debug!("RedisUserConfigStore::get_config - user config cache miss subject = {subject}");
            }
        }

        let Ok(key) = rmp_serde::encode::to_vec(key) else {
            warn!("RedisUserConfigStore::get_config - failed to encode Redis user config key subject = {subject}");
            return Err(ConfigStoreError::DataEncoding);
        };

        let mut connection = self.connection.clone();
        let maybe_user_config: Result<Option<Vec<u8>>, RedisError> =
            cmd("GET").arg(key).take().query_async(&mut connection).await;

        let user_config = match maybe_user_config {
            Ok(Some(user_config)) => {
                let bytes = user_config.len();
                debug!(
                    "RedisUserConfigStore::get_config - loaded user config blob from Redis subject = {subject} bytes = {bytes}"
                );
                user_config
            },
            Ok(None) => {
                debug!("RedisUserConfigStore::get_config - no user config found in Redis subject = {subject}");
                return Err(ConfigStoreError::NoDataForKey);
            },
            Err(error) => {
                warn!(
                    "RedisUserConfigStore::get_config - failed to load user config from Redis subject = {subject} error = {error}"
                );
                return Err(ConfigStoreError::NoDataForKey);
            },
        };

        let user_config = match rmp_serde::decode::from_slice::<V>(&user_config) {
            Ok(user_config) => user_config,
            Err(error) => {
                warn!(
                    "RedisUserConfigStore::get_config - failed to decode Redis user config blob subject = {subject} error = {error}"
                );
                return Err(ConfigStoreError::DataWrongFormat);
            },
        };

        debug!("RedisUserConfigStore::get_config - decoded user config subject = {subject}");

        if let Some(cache) = &self.cache {
            cache.lock().await.insert(subject.to_owned(), CachedEntry::new(user_config.clone()));
        }
        Ok(user_config)
    }
    pub async fn set_config<'a, K>(&self, key: &'a K, config: &'a V) -> Result<(), ConfigStoreError>
    where
        K: Key + Serialize,
    {
        let subject = key.key();

        let Ok(key) = rmp_serde::encode::to_vec(key) else {
            warn!("RedisUserConfigStore::set_config - failed to encode Redis user config key subject = {subject}");
            return Err(ConfigStoreError::DataEncoding);
        };

        let Ok(encoded) = rmp_serde::encode::to_vec(config) else {
            warn!("RedisUserConfigStore::set_config - failed to encode user config subject = {subject}");
            return Err(ConfigStoreError::DataEncoding);
        };

        let mut connection = self.connection.clone();

        match connection.set::<&[u8], &[u8], String>(&key, &encoded).await {
            Ok(_) => {
                let bytes = encoded.len();

                debug!(
                    "RedisUserConfigStore::set_config - wrote user config to Redis subject = {subject} bytes = {bytes}"
                );
                if let Some(cache) = &self.cache {
                    cache.lock().await.insert(subject.to_owned(), CachedEntry::new(config.clone()));
                }
                Ok(())
            },
            Err(error) => {
                warn!(
                    "RedisUserConfigStore::set_config - failed to write user config to Redis subject = {subject} error = {error}"
                );
                Err(ConfigStoreError::CantWriteData)
            },
        }
    }
}
