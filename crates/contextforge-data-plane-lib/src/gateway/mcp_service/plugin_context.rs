use std::{collections::HashSet, sync::Arc};

use contextforge_data_plane_apis::{
    User,
    user_store::{BackendMCPGateway, UserConfig},
};
use contextforge_data_plane_cpex::PluginRequestContext;
use cpex::cpex_core::extensions::{
    Extensions, HttpExtension, MCPExtension, MetaExtension, PromptMetadata, RequestExtension, ResourceMetadata,
    SecurityExtension, SubjectExtension, SubjectType, ToolMetadata,
};
use http::request::Parts;
use opentelemetry::trace::TraceContextExt;
use rmcp::{ErrorData, RoleServer, service::RequestContext};
use serde_json::Value;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::{
    authorization::{AuthorizationClaims, AuthorizedPrincipal},
    layers::virtual_host_id::VirtualHostId,
};

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
    let claims = parts
        .extensions
        .get::<AuthorizationClaims>()
        .ok_or_else(|| ErrorData::internal_error("Plugin verified claims are missing", None))?;
    let claims = Value::from(claims);
    let user_config = parts
        .extensions
        .get::<UserConfig>()
        .ok_or_else(|| ErrorData::internal_error("Plugin user configuration is missing", None))?;
    let server_id = parts.extensions.get::<VirtualHostId>().map(|id| id.value().clone());
    let tool = (kind == "tool").then(|| backend.tool_policy_contexts.get(name)).flatten().cloned();
    let canonical_name = tool.as_ref().map_or(name, |tool| tool.name.as_str());
    let user = User::from(principal);
    let subject = SubjectExtension {
        id: Some(user_config.user_email.as_deref().unwrap_or(user.key()).to_owned()),
        subject_type: Some(SubjectType::User),
        roles: string_set(claims.get("roles")),
        teams: string_set(claims.get("teams")),
        permissions: string_set(claims.get("scopes").and_then(|scopes| scopes.get("permissions"))),
        claims: claims
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), value.as_str().map_or_else(|| value.to_string(), str::to_owned)))
            .collect(),
    };
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
    let trace = tracing::Span::current().context();
    let span = trace.span();
    let span_context = span.span_context();
    Ok(PluginRequestContext {
        tool,
        extensions: Extensions {
            request: Some(Arc::new(RequestExtension {
                request_id: Some(uuid::Uuid::new_v4().to_string()),
                trace_id: span_context.is_valid().then(|| span_context.trace_id().to_string()),
                span_id: span_context.is_valid().then(|| span_context.span_id().to_string()),
                ..Default::default()
            })),
            http: Some(Arc::new(HttpExtension {
                request_headers: parts
                    .headers
                    .iter()
                    .filter_map(|(key, value)| value.to_str().ok().map(|value| (key.to_string(), value.to_owned())))
                    .collect(),
                method: Some(parts.method.to_string()),
                path: Some(parts.uri.path().to_owned()),
                host: parts.headers.get(http::header::HOST).and_then(|value| value.to_str().ok()).map(str::to_owned),
                scheme: parts.uri.scheme_str().map(str::to_owned),
                ..Default::default()
            })),
            security: Some(Arc::new(SecurityExtension { subject: Some(subject), ..Default::default() })),
            meta: Some(Arc::new(meta)),
            mcp: Some(Arc::new(mcp)),
            ..Default::default()
        },
    })
}

fn string_set(value: Option<&Value>) -> HashSet<String> {
    value.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}
