use std::collections::{HashMap, HashSet};

use cpex::cpex_core::config::CpexConfig;
use serde::{Deserialize, Deserializer, Serialize};

pub const RUNTIME_PLUGIN_CONFIG_KEY: &str = "ContextForgeGatewayRuntimePluginConfig";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimePluginConfigDocument {
    pub enabled: bool,
    #[serde(deserialize_with = "optional_config")]
    pub global: Option<CpexConfig>,
    #[serde(deserialize_with = "context_configs")]
    pub contexts: HashMap<String, CpexConfig>,
    #[serde(default)]
    pub settings: RuntimePluginSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimePluginSettings {
    /// Maximum seconds per plugin hook invocation; defaults to 30.
    pub plugin_timeout: u64,
    pub fail_on_plugin_error: bool,
    pub execution_pool: usize,
    pub default_hook_policy: String,
    pub hook_policies: HashMap<String, HookPayloadPolicy>,
    pub plugins_can_override_rbac: bool,
    pub plugins_can_override_auth_headers: bool,
}

impl Default for RuntimePluginSettings {
    fn default() -> Self {
        Self {
            plugin_timeout: 30,
            fail_on_plugin_error: false,
            execution_pool: 10,
            default_hook_policy: "allow".to_owned(),
            hook_policies: HashMap::new(),
            plugins_can_override_rbac: false,
            plugins_can_override_auth_headers: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookPayloadPolicy {
    pub writable_fields: HashSet<String>,
}

// The publisher serializes Python Config.plugins=None as JSON/MessagePack null.
// CPEX Rust represents the same empty configuration with an empty vector.
fn published_config<E: serde::de::Error>(mut value: serde_json::Value) -> Result<CpexConfig, E> {
    if value.get("plugins").is_some_and(serde_json::Value::is_null) {
        value["plugins"] = serde_json::json!([]);
    }
    serde_json::from_value(value).map_err(E::custom)
}

fn optional_config<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<CpexConfig>, D::Error> {
    Option::<serde_json::Value>::deserialize(deserializer)?.map(published_config).transpose()
}

fn context_configs<'de, D: Deserializer<'de>>(deserializer: D) -> Result<HashMap<String, CpexConfig>, D::Error> {
    HashMap::<String, serde_json::Value>::deserialize(deserializer)?
        .into_iter()
        .map(|(key, value)| published_config(value).map(|config| (key, config)))
        .collect()
}
