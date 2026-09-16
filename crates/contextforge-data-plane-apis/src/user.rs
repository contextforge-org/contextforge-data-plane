use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, Hash, PartialEq, PartialOrd, Ord, Eq, JsonSchema)]
enum KeyType {
    UserConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, Hash, PartialEq, PartialOrd, Ord, Eq, JsonSchema)]
pub struct User {
    name: KeyType,
    key: String,
}

impl User {
    pub fn key(&self) -> &str {
        self.key.as_str()
    }
}

impl User {
    pub fn new(key: &str) -> Self {
        Self { name: KeyType::UserConfig, key: key.to_owned() }
    }
}
