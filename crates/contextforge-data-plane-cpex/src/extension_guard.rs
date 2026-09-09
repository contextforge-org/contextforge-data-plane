//! Enforce the gateway's write boundary before CPEX merges each plugin result.
use std::{
    any::Any,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use cpex::cpex_core::{
    context::PluginContext,
    error::PluginError,
    executor::ErasedResultFields,
    hooks::{Extensions, PluginPayload},
    registry::{AnyHookHandler, HookEntry},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderWritePolicy {
    ReadOnly,
    PreserveCredentials,
    AllowNewCredentials,
    All,
}

struct ExtensionGuard {
    entry: HookEntry,
    canonical: Arc<Mutex<Extensions>>,
    payload_writable: bool,
    header_policy: HeaderWritePolicy,
}

pub(crate) fn guarded_entries(
    entries: &[HookEntry],
    extensions: &Extensions,
    payload_writable: bool,
    header_policy: HeaderWritePolicy,
) -> Vec<HookEntry> {
    let canonical = Arc::new(Mutex::new(extensions.clone()));
    entries
        .iter()
        .map(|entry| HookEntry {
            plugin_ref: Arc::clone(&entry.plugin_ref),
            handler: Arc::new(ExtensionGuard {
                entry: entry.clone(),
                canonical: Arc::clone(&canonical),
                payload_writable,
                header_policy,
            }),
        })
        .collect()
}

#[async_trait]
impl AnyHookHandler for ExtensionGuard {
    async fn invoke(
        &self,
        payload: &dyn PluginPayload,
        extensions: &Extensions,
        ctx: &mut PluginContext,
    ) -> Result<Box<dyn Any + Send + Sync>, Box<PluginError>> {
        let result = self.entry.handler.invoke(payload, extensions, ctx).await?;
        let mut result = result.downcast::<ErasedResultFields>().map_err(|_| {
            Box::new(PluginError::Config { message: "Plugin returned an invalid hook result".to_owned() })
        })?;
        if !self.payload_writable {
            result.modified_payload = None;
        }
        if let Some(modified) = result.modified_extensions.take()
            && self.entry.plugin_ref.mode().can_modify()
            && result.continue_processing
        {
            let mut canonical = self.canonical.lock().map_err(|_| {
                Box::new(PluginError::Config { message: "Plugin extension state is unavailable".to_owned() })
            })?;
            if canonical.validate_immutable(&modified) {
                let caps = &self.entry.plugin_ref.trusted_config().capabilities;
                let mut permitted = canonical.cow_copy();
                if self.header_policy != HeaderWritePolicy::ReadOnly
                    && caps.contains("write_headers")
                    && let Some(http) = modified.http
                {
                    let mut http = http.into_inner();
                    normalize_headers(
                        &mut http.request_headers,
                        canonical.http.as_ref().map(|http| &http.request_headers),
                    )?;
                    normalize_headers(
                        &mut http.response_headers,
                        canonical.http.as_ref().map(|http| &http.response_headers),
                    )?;
                    if self.header_policy != HeaderWritePolicy::All {
                        preserve_auth_headers(
                            &mut http.request_headers,
                            canonical.http.as_deref(),
                            self.header_policy == HeaderWritePolicy::AllowNewCredentials,
                        );
                    }
                    // A write-only plugin cannot return headers or metadata it never saw.
                    // Merge header edits while retaining the host's transport metadata.
                    let mut merged = permitted.http.take().unwrap_or_default().into_inner();
                    merged.request_headers.extend(http.request_headers);
                    merged.response_headers.extend(http.response_headers);
                    permitted.http = Some(cpex::cpex_core::extensions::Guarded::new(merged));
                }
                if let Some(updated) = modified.security
                    && let Some(security) = &mut permitted.security
                {
                    if caps.contains("append_labels") {
                        for label in updated.labels.iter() {
                            security.add_label(label.clone());
                        }
                    }
                    security.objects = updated.objects;
                    security.data = updated.data;
                    security.classification = updated.classification;
                }
                permitted.custom = modified.custom;
                canonical.merge_owned(permitted);
                result.modified_extensions = Some(canonical.cow_copy());
            }
        }
        Ok(result)
    }

    fn hook_type_name(&self) -> &'static str {
        self.entry.handler.hook_type_name()
    }
}

fn normalize_headers(
    headers: &mut std::collections::HashMap<String, String>,
    original: Option<&std::collections::HashMap<String, String>>,
) -> Result<(), Box<PluginError>> {
    let mut normalized = std::collections::HashMap::new();
    for (name, value) in std::mem::take(headers) {
        let name = name.to_ascii_lowercase();
        if let Some(previous) = normalized.get(&name) {
            if previous == &value {
                continue;
            }
            let original = original.and_then(|headers| {
                headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(&name)).map(|(_, value)| value)
            });
            if original == Some(&value) {
                continue;
            }
            if original != Some(previous) {
                return Err(Box::new(PluginError::Config {
                    message: "Plugin returned conflicting header values".to_owned(),
                }));
            }
        }
        normalized.insert(name, value);
    }
    *headers = normalized;
    Ok(())
}

fn preserve_auth_headers(
    headers: &mut std::collections::HashMap<String, String>,
    original: Option<&cpex::cpex_core::extensions::HttpExtension>,
    allow_new: bool,
) {
    fn protected(name: &str) -> bool {
        ["authorization", "proxy-authorization", "cookie", "x-api-key"]
            .iter()
            .any(|header| name.eq_ignore_ascii_case(header))
    }
    headers.retain(|name, _| {
        !protected(name) || (allow_new && !original.is_some_and(|http| http.has_request_header(name)))
    });
    if let Some(original) = original {
        headers.extend(
            original
                .request_headers
                .iter()
                .filter(|(name, _)| protected(name))
                .map(|(name, value)| (name.clone(), value.clone())),
        );
    }
}
