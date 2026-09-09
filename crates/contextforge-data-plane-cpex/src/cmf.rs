//! Common CMF envelopes and the response contract used by the hook runner.
//! Conversion rules stay with each operation because their MCP semantics differ.

use crate::hooks::Operation;
use cpex::cpex_core::{
    cmf::{ContentPart, Message, MessagePayload, Role, constants::SCHEMA_VERSION},
    executor::PipelineResult,
};
use rmcp::{ErrorData, model::ErrorCode};

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
