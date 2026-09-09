use contextforge_data_plane_cpex::PreHookResult;
use rmcp::{
    ErrorData, RoleServer,
    model::{ErrorCode, GetPromptRequestParams, GetPromptResponse},
    service::RequestContext,
};
use tracing::{info, instrument};

use super::McpService;
use crate::gateway::{
    mcp_call_validator::AuthorizedCallValidator, mcp_service::initialization::connect_backend_for_request,
    routing_error::backend_forward_error,
};

#[instrument(name = "get_prompt", level = "info", skip_all)]
pub(super) async fn get_prompt(
    mcp_service: &McpService,
    request: GetPromptRequestParams,
    cx: RequestContext<RoleServer>,
) -> Result<GetPromptResponse, ErrorData> {
    let mcp_call_validator = AuthorizedCallValidator::new("get_prompt", &cx);
    let (virtual_host, _claims) = mcp_call_validator.validate_stateless()?;
    let Some(route) = virtual_host.prompts.get(&request.name) else {
        return Err(ErrorData {
            code: ErrorCode::INVALID_PARAMS,
            message: "Routing problem... prompt not found".into(),
            data: None,
        });
    };

    let backend_name = route.backend_name.clone();
    let prompt_name = route.upstream_name.clone();

    let backend = virtual_host.backends.get(&backend_name).ok_or_else(|| ErrorData {
        code: ErrorCode::INVALID_PARAMS,
        message: "Routing problem... backend not found".into(),
        data: None,
    })?;
    let pre_result = if let Some(plugin_runtime) = &mcp_service.plugin_runtime {
        plugin_runtime
            .before_get_prompt(
                &request,
                &prompt_name,
                &backend_name,
                super::plugin_context::request_context(&cx, "prompt", &prompt_name, &backend_name, backend)?,
            )
            .await?
    } else {
        PreHookResult::default()
    };
    let mut backend_service = connect_backend_for_request(mcp_service, &backend_name, backend, &cx).await?;
    let mut routed_request = request;
    routed_request.name.clone_from(&prompt_name);
    pre_result.arguments.apply_to(&mut routed_request.arguments);
    let response = backend_service.get_prompt(routed_request).await;
    if let Err(error) = backend_service.close().await {
        tracing::warn!("get_prompt: backend cleanup failed backend_name = {backend_name} error = {error:?}");
    }
    let response = response.map_err(|error| backend_forward_error("get_prompt", &backend_name, &error))?;
    info!("get_prompt: backend {backend_name} returned {} messages", response.messages.len());
    let response = match pre_result.state {
        Some(state) => state.after_get_prompt(response).await?,
        None => response,
    };
    Ok(response.into())
}
