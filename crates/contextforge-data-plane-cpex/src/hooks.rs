use contextforge_data_plane_apis::user_store::ToolPolicyContext;
use cpex::cpex_core::hooks::{Extensions, types::cmf_hook_names};
use serde_json::{Map, Value};

/// Host-authored context from the authorized route and verified request.
/// Client metadata and plugin state never select a policy or establish identity.
#[derive(Default)]
pub struct PluginRequestContext {
    pub request: Option<crate::PluginRequest>,
    pub tool: Option<ToolPolicyContext>,
    /// With an existing request, only routed `meta` and `mcp` fields are applied.
    /// Transport data and verified identity belong to the shared request.
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

#[derive(Clone, Debug, Copy)]
pub(crate) enum Operation {
    Tool,
    Prompt,
    Resource,
    Http,
}

impl Operation {
    pub(crate) const MCP: [Self; 3] = [Self::Tool, Self::Prompt, Self::Resource];
    pub(crate) const ALL: [Self; 4] = [Self::Tool, Self::Prompt, Self::Resource, Self::Http];

    pub(crate) fn hooks(self) -> [&'static str; 2] {
        match self {
            Self::Tool => [cmf_hook_names::TOOL_PRE_INVOKE, cmf_hook_names::TOOL_POST_INVOKE],
            Self::Prompt => [cmf_hook_names::PROMPT_PRE_FETCH, cmf_hook_names::PROMPT_POST_FETCH],
            Self::Resource => [cmf_hook_names::RESOURCE_PRE_FETCH, cmf_hook_names::RESOURCE_POST_FETCH],
            Self::Http => ["http_pre_request", "http_post_request"],
        }
    }

    pub(crate) fn subject(self) -> &'static str {
        match self {
            Self::Tool => "tool call",
            Self::Prompt => "prompt",
            Self::Resource => "resource",
            Self::Http => "HTTP request",
        }
    }

    pub(crate) fn id_prefix(self) -> &'static str {
        match self {
            Self::Tool => "gateway-tool-call",
            Self::Prompt => "gateway-prompt-request",
            Self::Resource => "gateway-resource-request",
            Self::Http => "gateway-http-request",
        }
    }
}
