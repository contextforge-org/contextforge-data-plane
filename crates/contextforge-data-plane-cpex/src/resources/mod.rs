use base64::{Engine as _, prelude::BASE64_STANDARD};
use cpex::cpex_core::cmf::{
    ContentPart, MessagePayload, Resource as CmfResource, ResourceReference, ResourceType, Role,
};
use rmcp::{
    ErrorData,
    model::{ReadResourceResult, ResourceContents},
};

use crate::{
    GatewayPluginRuntimeHandle, PluginRequestContext,
    cmf::{CmfResponse, Operation, message_payload},
    runtime::CallState,
};

fn resource_request_payload(resource_uri: &str, resource_request_id: &str) -> MessagePayload {
    message_payload(
        Role::User,
        vec![ContentPart::ResourceRef {
            content: ResourceReference {
                resource_request_id: resource_request_id.to_owned(),
                uri: resource_uri.to_owned(),
                name: None,
                resource_type: ResourceType::Uri,
                range_start: None,
                range_end: None,
                selector: None,
            },
        }],
    )
}

fn resource_result_payload(response: &ReadResourceResult, resource_request_id: &str) -> Option<MessagePayload> {
    let content = response
        .contents
        .iter()
        .map(|content| {
            cmf_resource_content(content, resource_request_id).map(|content| ContentPart::Resource { content })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(message_payload(Role::Assistant, content))
}

fn cmf_resource_content(content: &ResourceContents, resource_request_id: &str) -> Option<CmfResource> {
    let (uri, mime_type, text, blob) = match content {
        ResourceContents::TextResourceContents { uri, mime_type, text, .. } => {
            (uri.clone(), mime_type.clone(), Some(text.clone()), None)
        },
        ResourceContents::BlobResourceContents { uri, mime_type, blob, .. } => {
            (uri.clone(), mime_type.clone(), None, Some(BASE64_STANDARD.decode(blob).ok()?))
        },
        _ => return None,
    };
    Some(CmfResource {
        resource_request_id: resource_request_id.to_owned(),
        uri,
        resource_type: ResourceType::Uri,
        content: text,
        blob,
        mime_type,
        ..Default::default()
    })
}

fn resource_result_response(mut original: ReadResourceResult, payload: &MessagePayload) -> Option<ReadResourceResult> {
    // Resource post hooks replace each resource's content, not the read envelope.
    if payload.message.content.len() != original.contents.len() {
        return None;
    }
    for (original, modified) in original.contents.iter_mut().zip(&payload.message.content) {
        let ContentPart::Resource { content } = modified else { return None };
        let meta = match original {
            ResourceContents::TextResourceContents { meta, .. }
            | ResourceContents::BlobResourceContents { meta, .. } => meta.clone(),
            _ => return None,
        };
        *original = match (&content.content, &content.blob) {
            (Some(text), _) => ResourceContents::TextResourceContents {
                uri: content.uri.clone(),
                mime_type: content.mime_type.clone(),
                text: text.clone(),
                meta,
            },
            (None, Some(bytes)) => {
                let blob = match original {
                    ResourceContents::BlobResourceContents { blob, .. }
                        if BASE64_STANDARD.decode(blob.as_bytes()).ok().as_ref() == Some(bytes) =>
                    {
                        blob.clone()
                    },
                    _ => BASE64_STANDARD.encode(bytes),
                };
                ResourceContents::BlobResourceContents {
                    uri: content.uri.clone(),
                    mime_type: content.mime_type.clone(),
                    blob,
                    meta,
                }
            },
            _ => return None,
        };
    }
    Some(original)
}

/// Captures the resource URI edit and post-hook decision for one request.
pub struct ResourceHookState {
    rewritten_uri: Option<String>,
    call: Option<CallState>,
}

impl ResourceHookState {
    pub fn rewritten_uri(&self) -> Option<&str> {
        self.rewritten_uri.as_deref()
    }

    pub async fn after_read_resource(self, response: ReadResourceResult) -> Result<ReadResourceResult, ErrorData> {
        match self.call {
            Some(mut call) => call.after(response).await,
            None => Ok(response),
        }
    }
}

impl GatewayPluginRuntimeHandle {
    pub async fn before_read_resource(
        &self,
        resource_uri: &str,
        context: PluginRequestContext,
    ) -> Result<ResourceHookState, ErrorData> {
        let (rewritten_uri, call) = self
            .current()?
            .global
            .before(
                (Operation::Resource, resource_uri),
                context.extensions,
                |id| resource_request_payload(resource_uri, id),
                |payload, _| {
                    let [ContentPart::ResourceRef { content }] = payload.message.content.as_slice() else {
                        return Err(ErrorData::internal_error("Plugin returned an invalid resource request", None));
                    };
                    Ok(Some(content.uri.clone()))
                },
            )
            .await?;
        Ok(ResourceHookState { rewritten_uri, call })
    }
}

impl CmfResponse for ReadResourceResult {
    const OPERATION: Operation = Operation::Resource;

    fn to_payload(&self, _name: &str, id: &str) -> Result<MessagePayload, ErrorData> {
        resource_result_payload(self, id)
            .ok_or_else(|| ErrorData::internal_error("Resource response contains an unsupported content type", None))
    }

    fn apply_payload(self, payload: &MessagePayload, _name: &str, _id: &str) -> Result<Self, ErrorData> {
        resource_result_response(self, payload).ok_or_else(|| {
            ErrorData::internal_error("Plugin returned a resource result the gateway cannot apply", None)
        })
    }
}

#[cfg(test)]
mod tests;
