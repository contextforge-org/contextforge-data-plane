use std::collections::HashMap;

use cpex::cpex_core::cmf::{
    AudioSource, ContentPart, ImageSource, Message, MessagePayload, PromptRequest, PromptResult,
    Resource as CmfResource, ResourceReference, ResourceType, Role,
};
use rmcp::{
    ErrorData,
    model::{
        ContentBlock, GetPromptRequestParams, GetPromptResult, PromptMessage, Resource as McpResource,
        ResourceContents, Role as McpRole,
    },
};
use serde_json::{Map, Value};

use crate::{
    ArgumentsUpdate, GatewayPluginRuntimeHandle, PreHookResult,
    cmf::{CmfResponse, Operation, message_payload},
    runtime::CallState,
};

fn prompt_request_payload(
    request: &GetPromptRequestParams,
    prompt_name: &str,
    backend_name: &str,
    prompt_request_id: &str,
) -> MessagePayload {
    message_payload(
        Role::User,
        vec![ContentPart::PromptRequest {
            content: PromptRequest {
                prompt_request_id: prompt_request_id.to_owned(),
                name: prompt_name.to_owned(),
                arguments: request.arguments.clone().map(HashMap::from_iter).unwrap_or_default(),
                server_id: Some(backend_name.to_owned()),
            },
        }],
    )
}

fn prompt_request_arguments(
    payload: &MessagePayload,
    prompt_name: &str,
    backend_name: &str,
    prompt_request_id: &str,
) -> Option<Map<String, Value>> {
    let requests = payload.message.get_prompt_requests();
    let [request] = requests.as_slice() else { return None };
    if request.name != prompt_name
        || request.prompt_request_id != prompt_request_id
        || request.server_id.as_deref() != Some(backend_name)
    {
        return None;
    }
    Some(request.arguments.clone().into_iter().collect::<Map<String, Value>>())
}

fn prompt_result_payload(response: &GetPromptResult, prompt_name: &str, prompt_request_id: &str) -> MessagePayload {
    let messages =
        response.messages.iter().map(|message| cmf_prompt_message(message, prompt_request_id)).collect::<Vec<_>>();

    message_payload(
        Role::Assistant,
        vec![ContentPart::PromptResult {
            content: PromptResult {
                prompt_request_id: prompt_request_id.to_owned(),
                prompt_name: prompt_name.to_owned(),
                messages,
                content: None,
                is_error: false,
                error_message: None,
            },
        }],
    )
}

fn prompt_result(payload: &MessagePayload) -> Option<&PromptResult> {
    let results = payload.message.get_prompt_results();
    let [result] = results.as_slice() else { return None };
    Some(*result)
}

fn prompt_result_rejection(payload: &MessagePayload) -> Option<String> {
    let result = prompt_result(payload)?;
    result
        .is_error
        .then(|| result.error_message.clone().unwrap_or_else(|| "Plugin rejected the rendered prompt".to_owned()))
}

// `None` means refuse: falling back to the backend's original would undo a plugin's redaction.
fn prompt_result_response(
    mut original: GetPromptResult,
    payload: &MessagePayload,
    prompt_name: &str,
    prompt_request_id: &str,
) -> Option<GetPromptResult> {
    let result = prompt_result(payload)?;
    if result.prompt_name != prompt_name
        || result.prompt_request_id != prompt_request_id
        || result.content.is_some()
        || result.error_message.is_some()
    {
        return None;
    }
    if result.messages.len() != original.messages.len() {
        return None;
    }

    for (message, edited) in original.messages.iter_mut().zip(&result.messages) {
        let projected = cmf_prompt_message(message, prompt_request_id);
        if serde_json::to_value(&projected).ok()? == serde_json::to_value(edited).ok()? {
            continue;
        }

        let rebuilt = mcp_prompt_message(edited)?;
        if serde_json::to_value(cmf_prompt_message(&rebuilt, prompt_request_id)).ok()?
            != serde_json::to_value(edited).ok()?
        {
            return None;
        }
        *message = rebuilt;
    }

    Some(original)
}

fn cmf_prompt_message(message: &PromptMessage, prompt_request_id: &str) -> Message {
    Message {
        schema_version: "2.0".to_owned(),
        role: match message.role {
            McpRole::Assistant => Role::Assistant,
            McpRole::User => Role::User,
        },
        content: cmf_content_part(&message.content, prompt_request_id).into_iter().collect(),
        channel: None,
    }
}

fn cmf_content_part(block: &ContentBlock, prompt_request_id: &str) -> Option<ContentPart> {
    let part = match block {
        ContentBlock::Text(text) => ContentPart::Text { text: text.text.clone() },
        ContentBlock::Image(image) => ContentPart::Image {
            content: ImageSource {
                source_type: "base64".to_owned(),
                data: image.data.clone(),
                media_type: Some(image.mime_type.clone()),
            },
        },
        ContentBlock::Audio(audio) => ContentPart::Audio {
            content: AudioSource {
                source_type: "base64".to_owned(),
                data: audio.data.clone(),
                media_type: Some(audio.mime_type.clone()),
                duration_ms: None,
            },
        },
        ContentBlock::Resource(resource) => {
            let (uri, mime_type, content) = match &resource.resource {
                ResourceContents::TextResourceContents { uri, mime_type, text, .. } => {
                    (uri.clone(), mime_type.clone(), Some(text.clone()))
                },
                ResourceContents::BlobResourceContents { uri, mime_type, .. } => (uri.clone(), mime_type.clone(), None),
                _ => return None,
            };
            ContentPart::Resource {
                content: CmfResource {
                    resource_request_id: prompt_request_id.to_owned(),
                    uri,
                    name: None,
                    description: None,
                    resource_type: ResourceType::Uri,
                    content,
                    blob: None,
                    mime_type,
                    size_bytes: None,
                    annotations: HashMap::new(),
                    version: None,
                },
            }
        },
        ContentBlock::ResourceLink(link) => ContentPart::ResourceRef {
            content: ResourceReference {
                resource_request_id: prompt_request_id.to_owned(),
                uri: link.uri.clone(),
                name: Some(link.name.clone()),
                resource_type: ResourceType::Uri,
                range_start: None,
                range_end: None,
                selector: None,
            },
        },
        _ => return None,
    };

    Some(part)
}

// MCP inlines image and audio bytes as base64, so a CMF source CMF can express but MCP cannot —
// a URL reference — has to be refused rather than written into a field that means something else.
fn inline_media_data<'a>(source_type: &str, data: &'a str) -> Option<&'a str> {
    (source_type == "base64").then_some(data)
}

fn mcp_prompt_message(message: &Message) -> Option<PromptMessage> {
    let role = match message.role {
        Role::Assistant => McpRole::Assistant,
        Role::User => McpRole::User,
        _ => return None,
    };

    let [part] = message.content.as_slice() else { return None };
    let content = match part {
        ContentPart::Text { text } => ContentBlock::text(text.clone()),
        ContentPart::Image { content } => {
            ContentBlock::image(inline_media_data(&content.source_type, &content.data)?, content.media_type.clone()?)
        },
        ContentPart::Audio { content } => {
            ContentBlock::audio(inline_media_data(&content.source_type, &content.data)?, content.media_type.clone()?)
        },
        ContentPart::Resource { content } => ContentBlock::resource(ResourceContents::TextResourceContents {
            uri: content.uri.clone(),
            mime_type: content.mime_type.clone(),
            text: content.content.clone()?,
            meta: None,
        }),
        ContentPart::ResourceRef { content } => {
            ContentBlock::ResourceLink(McpResource::new(content.uri.clone(), content.name.clone()?))
        },
        _ => return None,
    };

    Some(PromptMessage::new(role, content))
}

/// The runtime and plugin context captured before fetching one prompt.
pub struct PromptHookState(CallState);

impl GatewayPluginRuntimeHandle {
    pub async fn before_get_prompt(
        &self,
        request: &GetPromptRequestParams,
        prompt_name: &str,
        backend_name: &str,
    ) -> Result<PreHookResult<PromptHookState>, ErrorData> {
        let (arguments, state) = self
            .current()?
            .before(
                Operation::Prompt,
                prompt_name,
                |id| prompt_request_payload(request, prompt_name, backend_name, id),
                |payload, id| {
                    let arguments =
                        prompt_request_arguments(payload, prompt_name, backend_name, id).ok_or_else(|| {
                            ErrorData::invalid_params("Plugin returned a prompt request the gateway cannot apply", None)
                        })?;
                    Ok(ArgumentsUpdate::from_modified(request.arguments.as_ref(), arguments))
                },
            )
            .await?;
        Ok(PreHookResult { arguments, state: state.map(PromptHookState) })
    }
}

impl PromptHookState {
    pub async fn after_get_prompt(mut self, response: GetPromptResult) -> Result<GetPromptResult, ErrorData> {
        self.0.after(response).await
    }
}

impl CmfResponse for GetPromptResult {
    const OPERATION: Operation = Operation::Prompt;

    fn to_payload(&self, name: &str, id: &str) -> Result<MessagePayload, ErrorData> {
        Ok(prompt_result_payload(self, name, id))
    }

    fn apply_payload(self, payload: &MessagePayload, name: &str, id: &str) -> Result<Self, ErrorData> {
        if let Some(message) = prompt_result_rejection(payload) {
            return Err(ErrorData::invalid_request(message, None));
        }
        prompt_result_response(self, payload, name, id)
            .ok_or_else(|| ErrorData::internal_error("Plugin returned a prompt result the gateway cannot apply", None))
    }
}

#[cfg(test)]
mod tests;
