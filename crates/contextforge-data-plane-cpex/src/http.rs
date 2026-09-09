//! HTTP metadata is typed; header access and changes use capability-gated CPEX extensions.
use std::{net::SocketAddr, sync::Arc};

use cpex::cpex_core::{
    extensions::{Extensions, HttpExtension},
    hooks::PluginResult,
};
use rmcp::ErrorData;

use crate::{GatewayPluginRuntimeHandle, PluginRequest, PluginRequestContext, hooks::Operation};

#[derive(Clone, Debug)]
pub struct HttpHookPayload {
    pub method: String,
    pub path: String,
    pub client_addr: Option<SocketAddr>,
    /// Present only on the after hook. The hook runs before response headers are sent.
    pub status_code: Option<u16>,
}

cpex::cpex_core::impl_plugin_payload!(HttpHookPayload);
cpex::cpex_core::define_hook! {
    HttpHook, "http" => { payload: HttpHookPayload, result: PluginResult<HttpHookPayload> }
}

pub struct HttpHookState {
    pub request: PluginRequest,
    payload: HttpHookPayload,
    runtime: Arc<crate::runtime::GatewayPluginRuntime>,
}

impl GatewayPluginRuntimeHandle {
    pub async fn before_http_request(
        &self,
        payload: HttpHookPayload,
        extensions: Extensions,
    ) -> Result<HttpHookState, ErrorData> {
        let context = PluginRequestContext { extensions, ..Default::default() };
        let (request, runtime) = self.resolve(Operation::Http, &context)?;
        let state = HttpHookState { request, payload, runtime };
        state.invoke(0).await;
        Ok(state)
    }
}

impl HttpHookState {
    pub async fn after_http_request(
        mut self,
        status_code: u16,
        response_headers: std::collections::HashMap<String, String>,
    ) -> Extensions {
        self.payload.status_code = Some(status_code);
        {
            let mut state = self.request.state.lock().await;
            let http = state.extensions.http.get_or_insert_with(|| Arc::new(HttpExtension::default()));
            Arc::make_mut(http).response_headers = response_headers;
        }
        self.invoke(1).await;
        self.request.extensions().await
    }

    async fn invoke(&self, phase: usize) {
        // The same resolver and invocation engine serve all target kinds.
        let runtime = &self.runtime;
        let mut host = self.request.extensions().await;
        // The HTTP boundary has no routed MCP target, including during streaming.
        host.meta = None;
        host.mcp = None;
        let entries = runtime.entries(Operation::Http, phase, &host);
        let hook = Operation::Http.hooks()[phase];
        let result = runtime
            .invoke::<HttpHook>((Operation::Http, phase, &entries), self.payload.clone(), &self.request, &host)
            .await;
        if result.is_denied() {
            // Match the built-in HTTP middleware: hook failures do not reject HTTP requests.
            tracing::warn!(hook, "HTTP plugin hook did not complete; continuing request");
        }
    }
}
