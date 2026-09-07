use std::fs;
use std::path::Path;
use std::sync::Arc;

use cel::{Context, Program, objects::Key};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tracing::debug;

use crate::authorization::{AuthorizedPrincipal, PrincipalExtractor};

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

    #[error("Missing required field in CEL result: {0}")]
    MissingRequiredField(String),

    #[error("Invalid field type in CEL result: field={0}, expected={1}")]
    InvalidFieldType(String, String),
}

/// A CEL-based principal extractor that evaluates a CEL expression to extract
/// principal information from authorization claims.
///
/// The CEL expression should return a map with the following fields:
/// - `user_id` (string, required): The user identifier
/// - `tenant_id` (string, required): The tenant identifier
/// - `scopes` (list of strings, optional): The user's scopes/permissions
///
/// The CEL expression has access to the following variables:
/// - `claims`: A map containing all the authorization claims
/// - `sub`: The subject claim (shorthand for claims.sub)
/// - `tenant_id`: The tenant_id claim (shorthand for claims.tenant_id)
///
/// Example CEL expression:
/// ```cel
/// {
///   "user_id": claims.sub,
///   "tenant_id": claims.tenant_id,
///   "scopes": []
/// }
/// ```
#[derive(Clone)]
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

        debug!("CEL expression evaluated to: {:?}", result);

        Ok(AuthorizedPrincipal::try_from(result)?)
    }
}

impl TryFrom<cel::Value> for AuthorizedPrincipal {
    type Error = CelPrincipalExtractorError;

    fn try_from(value: cel::Value) -> Result<Self, Self::Error> {
        match value {
            cel::Value::Map(map) => {
                if let Some(cel::Value::String(user_id)) = map.get(&Key::from("user_id".to_owned()))
                    && let Some(cel::Value::String(tenant_id)) = map.get(&Key::from("tenant_id".to_owned()))
                {
                    let user_id = (**user_id).clone();
                    let tenant_id = (**tenant_id).clone();
                    Ok(AuthorizedPrincipal::builder().user_id(user_id).tenant_id(tenant_id).scopes(vec![]).build())
                } else {
                    Err(CelPrincipalExtractorError::InvalidReturnType(serde_json::Value::Null))
                }
            },
            _ => Err(CelPrincipalExtractorError::InvalidReturnType(serde_json::Value::Null)),
        }
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
