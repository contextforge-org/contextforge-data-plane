use async_trait::async_trait;
use contextforge_data_plane_apis::{User, user_store::UserConfig};

use super::ConfigStoreError;
use crate::config_stores::{ConfigStore, Key};

impl Key for User {
    fn key(&self) -> &str {
        self.key()
    }
}

#[async_trait]
impl ConfigStore<User, UserConfig> for super::RedisStore<UserConfig> {
    async fn get_config<'a>(&self, key: &'a User) -> Result<UserConfig, ConfigStoreError> {
        self.get_config(key).await
    }
    async fn set_config<'a>(&self, key: &'a User, config: &'a UserConfig) -> Result<(), ConfigStoreError> {
        self.set_config(key, config).await
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        time::{Duration, Instant},
    };

    use contextforge_data_plane_apis::user_store::UserConfig;
    use lru_time_cache::LruCache;

    use crate::{config_stores::CachedEntry, const_values::LRU_CACHE_ENTRIES};

    fn empty_config() -> UserConfig {
        UserConfig { virtual_hosts: HashMap::new() }
    }

    fn instant_ago(duration: Duration) -> Instant {
        Instant::now().checked_sub(duration).unwrap_or_else(Instant::now)
    }

    #[test]
    fn cache_entry_is_fresh_within_expiry() {
        let entry = CachedEntry::new(empty_config());

        assert!(entry.is_fresh(Duration::from_mins(1)));
    }

    #[test]
    fn cache_entry_expires_by_insert_time() {
        let expiry = Duration::from_mins(1);
        let entry = CachedEntry { inserted_at: instant_ago(expiry + Duration::from_secs(1)), config: empty_config() };

        assert!(!entry.is_fresh(expiry));
    }

    #[test]
    fn cache_access_does_not_renew_freshness() {
        let expiry = Duration::from_mins(1);
        let mut cache: LruCache<String, CachedEntry<UserConfig>> =
            LruCache::with_expiry_duration_and_capacity(expiry, LRU_CACHE_ENTRIES);
        cache.insert(
            "subject".to_owned(),
            CachedEntry { inserted_at: instant_ago(expiry + Duration::from_secs(1)), config: empty_config() },
        );

        for _ in 0..3 {
            let entry = cache.get_mut("subject").expect("entry should still be present in LruCache");
            assert!(!entry.is_fresh(expiry));
        }
    }
}
