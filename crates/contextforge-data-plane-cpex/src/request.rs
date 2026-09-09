//! One policy snapshot and CPEX state table for the whole HTTP/MCP request.
use std::{collections::HashMap, sync::Arc};

use cpex::cpex_core::{context::PluginContextTable, extensions::Extensions, registry::HookEntry};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::registry::RuntimePolicies;

#[derive(Clone)]
pub struct PluginRequest {
    pub(crate) policies: Arc<RuntimePolicies>,
    pub(crate) state: Arc<Mutex<RequestState>>,
}

impl PluginRequest {
    pub(crate) fn new(policies: Arc<RuntimePolicies>, extensions: Extensions) -> Self {
        Self { policies, state: Arc::new(Mutex::new(RequestState { extensions, ..Default::default() })) }
    }

    pub async fn set_verified_subject(&self, subject: cpex::cpex_core::extensions::SubjectExtension) {
        let mut state = self.state.lock().await;
        let security = state
            .extensions
            .security
            .get_or_insert_with(|| Arc::new(cpex::cpex_core::extensions::SecurityExtension::default()));
        Arc::make_mut(security).subject = Some(subject);
    }

    pub async fn sync_request_headers(&self, headers: HashMap<String, String>) {
        let mut state = self.state.lock().await;
        if let Some(http) = &mut state.extensions.http {
            Arc::make_mut(http).request_headers = headers;
        }
    }

    pub async fn extensions(&self) -> Extensions {
        self.state.lock().await.extensions.clone()
    }
}

#[derive(Default)]
pub(crate) struct RequestState {
    pub(crate) extensions: Extensions,
    pub(crate) contexts: PluginContextTable,
    // Global and scoped managers assign different IDs to the same configured plugin.
    // Move its CPEX local state when crossing that boundary; never share by name alone.
    plugin_ids: HashMap<(String, String), Uuid>,
}

impl RequestState {
    pub(crate) fn prepare(&mut self, entries: &[HookEntry]) {
        for entry in entries {
            let config = entry.plugin_ref.trusted_config();
            let id = entry.plugin_ref.id();
            if let Some(previous) = self.plugin_ids.insert((config.kind.clone(), config.name.clone()), id)
                && previous != id
                && let Some(local) = self.contexts.local_states.remove(&previous)
            {
                self.contexts.local_states.insert(id, local);
            }
        }
    }
}
