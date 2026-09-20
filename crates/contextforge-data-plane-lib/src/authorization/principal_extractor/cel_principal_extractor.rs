use std::fs;
use std::path::Path;
use std::sync::Arc;

use cel::{Context, Program, objects::Key};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tracing::instrument;

use super::{AuthorizedPrincipal, PrincipalConfig, PrincipalExtractor, UserClaim};

#[derive(Error, Debug)]
pub enum CelPrincipalExtractorError {
    #[error("Failed to read CEL expression file: {0}")]
    FileReadError(#[from] std::io::Error),

    #[error("Failed to compile CEL expression: {0}")]
    CompilationError(String),

    #[error("Failed to evaluate CEL expression: {0}")]
    EvaluationError(String),

    #[error("CEL expression did not return a map: {0:?}")]
    InvalidReturnType(JsonValue),
}

/// Trusted CEL mapping for nonstandard claims. Return `user_id`, `tenant_id`,
/// and optional `role`, `roles`, `scope`, `permissions`. Permissions are always
/// computed by the same policy as the default extractor. The verified issuer
/// is taken from the original claims and cannot be overridden by CEL.
#[derive(Clone, Debug)]
pub struct CelPrincipalExtractor {
    program: Arc<Program>,
    config: PrincipalConfig,
}

impl CelPrincipalExtractor {
    pub fn with_config(mut self, mut config: PrincipalConfig) -> Self {
        config.user_claim = UserClaim::Sub;
        self.config = config;
        self
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, CelPrincipalExtractorError> {
        let expression = fs::read_to_string(path)?;
        Self::from_expression(&expression)
    }

    pub fn from_expression(expression: &str) -> Result<Self, CelPrincipalExtractorError> {
        let program =
            Program::compile(expression).map_err(|e| CelPrincipalExtractorError::CompilationError(e.to_string()))?;

        Ok(Self { program: Arc::new(program), config: PrincipalConfig::default() })
    }
}

impl PrincipalExtractor for CelPrincipalExtractor {
    #[instrument(name = "principal_extract", level = "info", skip_all)]
    fn extract(
        &self,
        claims: &serde_json::Value,
    ) -> Result<AuthorizedPrincipal, Box<dyn std::error::Error + Send + Sync>> {
        let mut context = Context::default();

        context
            .add_variable("claims", claims)
            .map_err(|e| CelPrincipalExtractorError::EvaluationError(format!("Failed to add claims variable: {e}")))?;

        let result =
            self.program.execute(&context).map_err(|e| CelPrincipalExtractorError::EvaluationError(e.to_string()))?;

        let cel::Value::Map(map) = result else {
            return Err(CelPrincipalExtractorError::InvalidReturnType(JsonValue::Null).into());
        };
        let mut normalized = serde_json::Map::new();
        for name in ["user_id", "tenant_id", "role", "roles", "scope", "permissions"] {
            if let Some(value) = map.get(&Key::from(name.to_owned())) {
                let value = match value {
                    cel::Value::String(value) => JsonValue::String(value.to_string()),
                    cel::Value::List(values) => JsonValue::Array(
                        values
                            .iter()
                            .map(|value| {
                                if let cel::Value::String(value) = value {
                                    JsonValue::String(value.to_string())
                                } else {
                                    JsonValue::Null
                                }
                            })
                            .collect(),
                    ),
                    _ => JsonValue::Null,
                };
                normalized.insert(if name == "user_id" { "sub" } else { name }.to_owned(), value);
            }
        }
        normalized.insert("iss".to_owned(), claims.get("iss").cloned().unwrap_or(JsonValue::Null));
        Ok(AuthorizedPrincipal::from_claims(&JsonValue::Object(normalized), &self.config)?)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn create_test_claims() -> serde_json::Value {
        json!( {
                "sub": "user123",
                "tenant_id": "tenant456",
                "iss": "https://auth.example.com",
                "aud": "api",
                "exp": "1234567890",
                "iat": "1234567800"
        })
    }

    #[test]
    fn test_extractor_creation() {
        // Test that the extractor can be created from a valid CEL expression
        let expression = r#"
        {
            "user_id": claims.sub,
            "tenant_id": claims.tenant_id,
            "scopes": []
        }
        "#;

        let result = CelPrincipalExtractor::from_expression(expression);
        assert!(result.is_ok(), "Should compile valid CEL expression");
    }

    #[test]
    fn test_parse_valid_expression() {
        let expression = r#"{"user_id": "test", "tenant_id": "tenant"}"#;
        let result = CelPrincipalExtractor::from_expression(expression);
        assert!(result.is_ok());
    }

    #[test]
    fn test_missing_required_field() {
        let expression = r#"
        {
            "user_id": claims.sub,
            "scopes": []
        }
        "#;

        let extractor = CelPrincipalExtractor::from_expression(expression).expect("Should compile");
        let claims = create_test_claims();
        let result = extractor.extract(&claims);

        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_return_type() {
        let expression = r#""not a map""#;

        let extractor = CelPrincipalExtractor::from_expression(expression).expect("Should compile");
        let claims = create_test_claims();
        let result = extractor.extract(&claims);

        assert!(result.is_err());
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    use crate::Permission;

    #[test]
    fn nested_roles_use_common_policy_and_cannot_override_issuer() {
        let extractor = CelPrincipalExtractor::from_expression(r#"{"user_id": claims.sub, "tenant_id": claims.woTenantId, "roles": claims.user.roles, "iss": "untrusted"}"#).unwrap();
        let principal = extractor
            .extract(&serde_json::json!({"sub":"user","woTenantId":"tenant","iss":"watson","user":{"roles":["admin"]}}))
            .unwrap();
        assert_eq!(principal.issuer(), "watson");
        assert!(principal.has_permission(Permission::Admin));
        let extractor =
            CelPrincipalExtractor::from_expression(r#"{"user_id": claims.sub, "tenant_id": claims.woTenantId}"#)
                .unwrap();
        let principal = extractor
            .extract(&serde_json::json!({"sub":"user","woTenantId":"tenant","iss":"watson","role":"admin"}))
            .unwrap();
        assert!(!principal.has_permission(Permission::MCPUser));
    }
}
