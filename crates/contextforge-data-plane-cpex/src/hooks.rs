use contextforge_data_plane_apis::user_store::ToolPolicyContext;
use cpex::cpex_core::hooks::Extensions;
use serde_json::{Map, Value};

/// Host-authored context from the authorized route and verified request.
/// Client metadata and plugin state never select a policy or establish identity.
#[derive(Default)]
pub struct PluginRequestContext {
    pub tool: Option<ToolPolicyContext>,
    pub extensions: Extensions,
}

pub type RuntimeHookError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Default)]
pub enum ArgumentsUpdate {
    #[default]
    Unchanged,
    Replace(Option<Map<String, Value>>),
}

impl ArgumentsUpdate {
    pub(crate) fn from_modified(original: Option<&Map<String, Value>>, modified: Map<String, Value>) -> Self {
        if original == Some(&modified) || (original.is_none() && modified.is_empty()) {
            Self::Unchanged
        } else {
            Self::Replace(Some(modified))
        }
    }

    pub fn apply_to(self, arguments: &mut Option<Map<String, Value>>) {
        if let Self::Replace(replacement) = self {
            *arguments = replacement;
        }
    }
}

/// Argument edits and the typed post-hook state captured before backend I/O.
pub struct PreHookResult<S> {
    pub arguments: ArgumentsUpdate,
    pub state: Option<S>,
}

impl<S> Default for PreHookResult<S> {
    fn default() -> Self {
        Self { arguments: ArgumentsUpdate::Unchanged, state: None }
    }
}
