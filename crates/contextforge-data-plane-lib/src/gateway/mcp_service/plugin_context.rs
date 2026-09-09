use std::sync::Arc;

use contextforge_data_plane_apis::user_store::BackendMCPGateway;
use contextforge_data_plane_cpex::{PluginRequest, PluginRequestContext};
use cpex::cpex_core::extensions::{
    Extensions, MCPExtension, MetaExtension, PromptMetadata, ResourceMetadata, ToolMetadata,
};
use http::request::Parts;
use rmcp::{ErrorData, RoleServer, service::RequestContext};
use serde_json::Value;

use crate::{authorization::AuthorizedPrincipal, layers::virtual_host_id::VirtualHostId};

/// Construct plugin input only after authorization and route resolution. Nothing
/// in MCP arguments or client metadata participates in identity or policy lookup.
pub(super) fn request_context(
    cx: &RequestContext<RoleServer>,
    kind: &str,
    name: &str,
    backend_name: &str,
    backend: &BackendMCPGateway,
) -> Result<PluginRequestContext, ErrorData> {
    let parts = cx
        .extensions
        .get::<Parts>()
        .ok_or_else(|| ErrorData::internal_error("Plugin request context is missing", None))?;
    let principal = parts
        .extensions
        .get::<AuthorizedPrincipal>()
        .ok_or_else(|| ErrorData::internal_error("Plugin verified principal is missing", None))?;
    let request = parts
        .extensions
        .get::<PluginRequest>()
        .ok_or_else(|| ErrorData::internal_error("Plugin request state is missing", None))?;
    let server_id = parts.extensions.get::<VirtualHostId>().map(|id| id.value().clone());
    let tool = (kind == "tool").then(|| backend.tool_policy_contexts.get(name)).flatten().cloned();
    let canonical_name = tool.as_ref().map_or(name, |tool| tool.name.as_str());
    let tenant = tool.as_ref().and_then(|tool| tool.team_id.as_deref()).unwrap_or(principal.tenant_id());
    let mut meta = MetaExtension {
        entity_type: Some(kind.to_owned()),
        entity_name: Some(canonical_name.to_owned()),
        scope: Some(tenant.to_owned()),
        ..Default::default()
    };
    if let Some(tool) = &tool {
        meta.properties.insert("tool_id".to_owned(), tool.id.clone());
        meta.properties.insert("context_id".to_owned(), tool.context_id.clone());
    }
    meta.properties.insert("backend_id".to_owned(), backend_name.to_owned());
    if let Some(server_id) = &server_id {
        meta.properties.insert("virtual_server_id".to_owned(), server_id.clone());
    }
    let mcp = match kind {
        "tool" => MCPExtension {
            tool: Some(ToolMetadata {
                name: canonical_name.to_owned(),
                server_id: Some(backend_name.to_owned()),
                namespace: Some(backend_name.to_owned()),
                input_schema: backend.tool_schemas.get(name).cloned().map(Value::Object),
                ..Default::default()
            }),
            ..Default::default()
        },
        "prompt" => MCPExtension {
            prompt: Some(PromptMetadata { name: name.to_owned(), server_id, ..Default::default() }),
            ..Default::default()
        },
        _ => MCPExtension {
            resource: Some(ResourceMetadata { uri: name.to_owned(), server_id, ..Default::default() }),
            ..Default::default()
        },
    };
    Ok(PluginRequestContext {
        request: Some(request.clone()),
        tool,
        extensions: Extensions { meta: Some(Arc::new(meta)), mcp: Some(Arc::new(mcp)), ..Default::default() },
    })
}
