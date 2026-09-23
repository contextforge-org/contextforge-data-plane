mod cel_principal_extractor;
mod default_principal_extractor;

use std::collections::BTreeSet;

use contextforge_data_plane_apis::User;
use serde_json::Value;

pub use cel_principal_extractor::CelPrincipalExtractor;
pub use default_principal_extractor::DefaultPrincipalExtractor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    Admin,
    MCPUser,
}

#[derive(Debug, thiserror::Error)]
pub enum PrincipalError {
    #[error("missing or malformed identity claim")]
    InvalidIdentity,
    #[error("conflicting tenant claims")]
    ConflictingTenant,
    #[error("malformed role claims")]
    InvalidPermissions,
}

/// Request-local identity derived only from verified claims and trusted mapping configuration.
/// This is not yet a tenant-aware persistent configuration key.
#[derive(Clone)]
pub struct AuthorizedPrincipal {
    issuer: String,
    user_id: String,
    tenant_id: String,
    permissions: BTreeSet<Permission>,
}

impl std::fmt::Debug for AuthorizedPrincipal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedPrincipal").field("permissions", &self.permissions).finish_non_exhaustive()
    }
}

impl AuthorizedPrincipal {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub fn has_permission(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    fn from_claims(claims: &Value) -> Result<Self, PrincipalError> {
        let issuer = identity_string(claims.get("iss"))?;
        let user_id = identity_string(claims.get("sub"))?;
        let mut tenant_id = None;
        for name in ["woTenantId", "tenant_id", "tenantId"] {
            if let Some(value) = claims.get(name) {
                let value = identity_string(Some(value))?;
                if tenant_id.is_some_and(|tenant| tenant != value) {
                    return Err(PrincipalError::ConflictingTenant);
                }
                tenant_id = Some(value);
            }
        }
        let tenant_id = tenant_id.ok_or(PrincipalError::InvalidIdentity)?;
        let mut roles = string_list(claims.get("roles"))?;
        if let Some(role) = claims.get("role") {
            roles.push(role.as_str().ok_or(PrincipalError::InvalidPermissions)?.to_owned());
        }
        let mut permissions = BTreeSet::new();
        for role in roles {
            match role.as_str() {
                "admin" => {
                    permissions.extend([Permission::Admin, Permission::MCPUser]);
                },
                "builder" | "user" => {
                    permissions.insert(Permission::MCPUser);
                },
                _ => {},
            }
        }
        Ok(Self {
            issuer: issuer.to_owned(),
            user_id: user_id.to_owned(),
            tenant_id: tenant_id.to_owned(),
            permissions,
        })
    }
}

fn identity_string(value: Option<&Value>) -> Result<&str, PrincipalError> {
    value.and_then(Value::as_str).filter(|value| !value.trim().is_empty()).ok_or(PrincipalError::InvalidIdentity)
}

fn string_list(value: Option<&Value>) -> Result<Vec<String>, PrincipalError> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| value.as_str().map(str::to_owned).ok_or(PrincipalError::InvalidPermissions))
            .collect(),
        _ => Err(PrincipalError::InvalidPermissions),
    }
}

impl From<&AuthorizedPrincipal> for User {
    fn from(value: &AuthorizedPrincipal) -> Self {
        // The publisher and ConfigStore still use subject-only keys. Tenant isolation
        // requires a coordinated schema/publisher migration in the next change.
        Self::new(&value.user_id)
    }
}

pub trait PrincipalExtractor {
    fn extract(&self, claims: &Value) -> Result<AuthorizedPrincipal, Box<dyn std::error::Error + Send + Sync>>;
}
