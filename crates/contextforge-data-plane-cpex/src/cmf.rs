//! Common CMF envelopes and the response contract used by the hook runner.
//! Conversion rules stay with each operation because their MCP semantics differ.

use cpex::cpex_core::{
    cmf::{ContentPart, Message, MessagePayload, Role, constants::SCHEMA_VERSION},
    executor::PipelineResult,
    hooks::types::cmf_hook_names,
};
use rmcp::{ErrorData, model::ErrorCode};

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Tool,
    Prompt,
    Resource,
}

impl Operation {
    pub(crate) const ALL: [Self; 3] = [Self::Tool, Self::Prompt, Self::Resource];

    pub(crate) fn hooks(self) -> [&'static str; 2] {
        match self {
            Self::Tool => [cmf_hook_names::TOOL_PRE_INVOKE, cmf_hook_names::TOOL_POST_INVOKE],
            Self::Prompt => [cmf_hook_names::PROMPT_PRE_FETCH, cmf_hook_names::PROMPT_POST_FETCH],
            Self::Resource => [cmf_hook_names::RESOURCE_PRE_FETCH, cmf_hook_names::RESOURCE_POST_FETCH],
        }
    }

    pub(crate) fn subject(self) -> &'static str {
        match self {
            Self::Tool => "tool call",
            Self::Prompt => "prompt",
            Self::Resource => "resource",
        }
    }

    pub(crate) fn id_prefix(self) -> &'static str {
        match self {
            Self::Tool => "gateway-tool-call",
            Self::Prompt => "gateway-prompt-request",
            Self::Resource => "gateway-resource-request",
        }
    }
}

/// Only the projection and application differ between response types.
/// The runner owns invocation, unchanged payloads, context, and denial handling.
pub(crate) trait CmfResponse: Sized {
    const OPERATION: Operation;

    fn to_payload(&self, name: &str, id: &str) -> Result<MessagePayload, ErrorData>;
    fn apply_payload(self, payload: &MessagePayload, name: &str, id: &str) -> Result<Self, ErrorData>;
}

pub(crate) fn message_payload(role: Role, content: Vec<ContentPart>) -> MessagePayload {
    MessagePayload { message: Message { schema_version: SCHEMA_VERSION.to_owned(), role, content, channel: None } }
}

pub(crate) fn modified_message_payload(result: &PipelineResult) -> Option<&MessagePayload> {
    result.modified_payload.as_ref().and_then(|payload| payload.as_any().downcast_ref::<MessagePayload>())
}

pub(crate) fn plugin_denied_error(subject: &str, result: PipelineResult) -> ErrorData {
    let code = result
        .violation
        .and_then(|violation| {
            tracing::warn!("Plugin denied {subject}: code={} plugin={:?}", violation.code, violation.plugin_name);
            violation.proto_error_code.and_then(|code| i32::try_from(code).ok()).map(ErrorCode)
        })
        .unwrap_or(ErrorCode::INVALID_REQUEST);

    ErrorData { code, message: format!("Plugin denied {subject}").into(), data: None }
}
