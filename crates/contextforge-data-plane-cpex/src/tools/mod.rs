use std::sync::Arc;

use cpex::cpex_core::cmf::{ContentPart, MessagePayload, Role, ToolCall, ToolResult};
use rmcp::{
    ErrorData,
    model::{CallToolRequestParams, CallToolResult, ContentBlock},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::{
    ArgumentsUpdate, GatewayPluginRuntimeHandle, PluginRequestContext, PreHookResult,
    cmf::{CmfResponse, Operation, message_payload},
    runtime::CallState,
};

fn tool_call_payload(
    request: &CallToolRequestParams,
    tool_name: &str,
    backend_name: &str,
    tool_call_id: &str,
) -> MessagePayload {
    message_payload(
        Role::Assistant,
        vec![ContentPart::ToolCall {
            content: ToolCall {
                tool_call_id: tool_call_id.to_owned(),
                name: tool_name.to_owned(),
                arguments: request.arguments.clone().unwrap_or_default().into_iter().collect(),
                namespace: Some(backend_name.to_owned()),
            },
        }],
    )
}

fn tool_result_payload(tool_name: &str, response: &CallToolResult, tool_call_id: &str) -> MessagePayload {
    tool_json_result_payload(
        tool_name,
        serde_json::to_value(response).unwrap_or(Value::Null),
        response.is_error.unwrap_or(false),
        tool_call_id,
    )
}

fn tool_json_result_payload(tool_name: &str, content: Value, is_error: bool, tool_call_id: &str) -> MessagePayload {
    message_payload(
        Role::Tool,
        vec![ContentPart::ToolResult {
            content: ToolResult {
                tool_call_id: tool_call_id.to_owned(),
                tool_name: tool_name.to_owned(),
                content,
                is_error,
            },
        }],
    )
}

fn tool_result_content(payload: &MessagePayload) -> Option<Value> {
    payload.message.get_tool_results().first().map(|tool_result| tool_result.content.clone())
}

fn tool_call_arguments(payload: &MessagePayload) -> Option<Map<String, Value>> {
    payload
        .message
        .get_tool_calls()
        .first()
        .map(|tool_call| tool_call.arguments.clone().into_iter().collect::<Map<String, Value>>())
}

fn tool_result_response(original: CallToolResult, payload: &MessagePayload) -> CallToolResult {
    let mut result = payload.message.get_tool_results().first().map_or(original, |tool_result| {
        serde_json::from_value::<CallToolResult>(tool_result.content.clone()).map_or_else(
            |_| raw_tool_result(tool_result.content.clone(), tool_result.is_error),
            |mut result| {
                result.is_error = Some(tool_result.is_error);
                result
            },
        )
    });

    let text = payload.message.get_text_content();
    if !text.is_empty() {
        result.content.push(ContentBlock::text(text));
    }

    result
}

fn raw_tool_result(value: Value, is_error: bool) -> CallToolResult {
    match (value, is_error) {
        (Value::String(text), false) => CallToolResult::success(vec![ContentBlock::text(text)]),
        (Value::String(text), true) => CallToolResult::error(vec![ContentBlock::text(text)]),
        (value, false) => CallToolResult::structured(value),
        (value, true) => CallToolResult::structured_error(value),
    }
}

/// Shared by the final tool response and its progress notifications.
#[derive(Clone)]
pub struct ToolHookState(Arc<Mutex<CallState>>);

impl std::fmt::Debug for ToolHookState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolHookState").finish_non_exhaustive()
    }
}

impl GatewayPluginRuntimeHandle {
    pub async fn before_tool_call(
        &self,
        request: &CallToolRequestParams,
        tool_name: &str,
        backend_name: &str,
        context: PluginRequestContext,
    ) -> Result<PreHookResult<ToolHookState>, ErrorData> {
        let (arguments, state) = self
            .current()?
            .tool(context.tool.as_ref())?
            .before(
                (Operation::Tool, tool_name),
                context.extensions,
                |id| tool_call_payload(request, tool_name, backend_name, id),
                |payload, _| {
                    let arguments = tool_call_arguments(payload).ok_or_else(|| {
                        ErrorData::invalid_params("Plugin modified tool payload without a tool call", None)
                    })?;
                    Ok(ArgumentsUpdate::from_modified(request.arguments.as_ref(), arguments))
                },
            )
            .await?;
        Ok(PreHookResult { arguments, state: state.map(|state| ToolHookState(Arc::new(Mutex::new(state)))) })
    }
}

impl ToolHookState {
    pub async fn after_tool_call(self, response: CallToolResult) -> Result<CallToolResult, ErrorData> {
        self.0.lock().await.after(response).await
    }

    /// Returns `None` when a plugin denies a progress or logging notification.
    pub async fn after_stream_event<T>(&self, event: T) -> Result<Option<T>, ErrorData>
    where
        T: Serialize + DeserializeOwned,
    {
        let mut state = self.0.lock().await;
        let event = ToolEvent(event);
        let result = state.invoke(&event).await?;
        if result.is_denied() {
            return Ok(None);
        }
        Ok(Some(state.apply(event, &result)?.0))
    }
}

impl CmfResponse for CallToolResult {
    const OPERATION: Operation = Operation::Tool;

    fn to_payload(&self, name: &str, id: &str) -> Result<MessagePayload, ErrorData> {
        Ok(tool_result_payload(name, self, id))
    }

    fn apply_payload(self, payload: &MessagePayload, _name: &str, _id: &str) -> Result<Self, ErrorData> {
        Ok(tool_result_response(self, payload))
    }
}

struct ToolEvent<T>(T);

impl<T: Serialize + DeserializeOwned> CmfResponse for ToolEvent<T> {
    const OPERATION: Operation = Operation::Tool;

    fn to_payload(&self, name: &str, id: &str) -> Result<MessagePayload, ErrorData> {
        Ok(tool_json_result_payload(name, serde_json::to_value(&self.0).unwrap_or(Value::Null), false, id))
    }

    fn apply_payload(self, payload: &MessagePayload, _name: &str, _id: &str) -> Result<Self, ErrorData> {
        let content = tool_result_content(payload).ok_or_else(|| {
            ErrorData::invalid_params("Plugin modified stream event payload without a tool result", None)
        })?;
        serde_json::from_value(content).map(Self).map_err(|error| {
            ErrorData::invalid_params(format!("Plugin modified stream event payload with invalid JSON: {error}"), None)
        })
    }
}

#[cfg(test)]
mod tests;
