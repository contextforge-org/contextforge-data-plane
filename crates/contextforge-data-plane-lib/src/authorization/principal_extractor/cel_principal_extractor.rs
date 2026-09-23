use std::fs;
use std::path::Path;
use std::sync::Arc;

use cel::{Context, Program, objects::Key};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tracing::instrument;

use super::{AuthorizedPrincipal, PrincipalExtractor};

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

/// CEL maps user_id and tenant_id. Roles and issuer always come from the
/// original verified claims and use the same policy as the default extractor.
#[derive(Clone, Debug)]
pub struct CelPrincipalExtractor {
    program: Arc<Program>,
}

impl CelPrincipalExtractor {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, CelPrincipalExtractorError> {
        let expression = fs::read_to_string(path)?;
        Self::from_expression(&expression)
    }

    pub fn from_expression(expression: &str) -> Result<Self, CelPrincipalExtractorError> {
        let program =
            Program::compile(expression).map_err(|e| CelPrincipalExtractorError::CompilationError(e.to_string()))?;

        Ok(Self { program: Arc::new(program) })
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
        for (name, claim) in [("user_id", "sub"), ("tenant_id", "tenant_id")] {
            let value = match map.get(&Key::from(name.to_owned())) {
                Some(cel::Value::String(value)) => JsonValue::String(value.to_string()),
                _ => JsonValue::Null,
            };
            normalized.insert(claim.to_owned(), value);
        }
        for name in ["iss", "role", "roles"] {
            if let Some(value) = claims.get(name) {
                normalized.insert(name.to_owned(), value.clone());
            }
        }
        Ok(AuthorizedPrincipal::from_claims(&JsonValue::Object(normalized))?)
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
    fn mapping_cannot_override_verified_roles_or_issuer() {
        let extractor = CelPrincipalExtractor::from_expression(
            r#"{"user_id": claims.sub, "tenant_id": claims.woTenantId, "role": "admin", "iss": "other"}"#,
        )
        .unwrap();
        let claims = serde_json::json!({"sub":"user","woTenantId":"tenant","iss":"watson","roles":["user"]});
        let principal = extractor.extract(&claims).unwrap();
        assert_eq!(principal.issuer(), "watson");
        assert!(principal.has_permission(Permission::MCPUser));
        assert!(!principal.has_permission(Permission::Admin));
        let mut no_roles = claims;
        no_roles.as_object_mut().unwrap().remove("roles");
        assert!(!extractor.extract(&no_roles).unwrap().has_permission(Permission::MCPUser));
    }
}
