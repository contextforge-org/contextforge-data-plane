use std::sync::Arc;

use cpex::cpex_core::{
    cmf::CmfHook,
    error::PluginError,
    factory::{PluginFactory, PluginInstance},
    hooks::{HookHandler, TypedHandlerAdapter},
    plugin::{Plugin, PluginConfig},
    registry::AnyHookHandler,
};

use crate::{HttpHook, hooks::Operation};

type HandlerFactory<P> = fn(Arc<P>) -> Arc<dyn AnyHookHandler>;

/// Registers HTTP and/or MCP handlers on the same native plugin instance.
#[must_use]
pub struct GatewayPluginFactory<P> {
    build: Box<dyn Fn(PluginConfig) -> P + Send + Sync>,
    handlers: Vec<(&'static str, HandlerFactory<P>)>,
}

impl<P: Plugin + 'static> GatewayPluginFactory<P> {
    pub fn new(build: impl Fn(PluginConfig) -> P + Send + Sync + 'static) -> Self {
        Self { build: Box::new(build), handlers: Vec::new() }
    }

    pub fn with_cmf_hooks(mut self) -> Self
    where
        P: HookHandler<CmfHook>,
    {
        self.handlers.extend(Operation::MCP.into_iter().flat_map(Operation::hooks).map(|name| {
            (
                name,
                (|plugin| Arc::new(TypedHandlerAdapter::<CmfHook, P>::new(plugin)) as Arc<dyn AnyHookHandler>)
                    as HandlerFactory<P>,
            )
        }));
        self
    }

    pub fn with_http_hooks(mut self) -> Self
    where
        P: HookHandler<HttpHook>,
    {
        self.handlers.extend(Operation::Http.hooks().map(|name| {
            (
                name,
                (|plugin| Arc::new(TypedHandlerAdapter::<HttpHook, P>::new(plugin)) as Arc<dyn AnyHookHandler>)
                    as HandlerFactory<P>,
            )
        }));
        self
    }
}

impl<P: Plugin + 'static> PluginFactory for GatewayPluginFactory<P> {
    fn create(&self, config: &PluginConfig) -> Result<PluginInstance, Box<PluginError>> {
        let plugin = Arc::new((self.build)(config.clone()));
        let handlers = config
            .hooks
            .iter()
            .map(|hook| {
                let (name, build) = self.handlers.iter().find(|(name, _)| *name == hook).ok_or_else(|| {
                    Box::new(PluginError::Config {
                        message: format!("plugin '{}' has no handler for '{hook}'", config.name),
                    })
                })?;
                Ok((*name, build(Arc::clone(&plugin))))
            })
            .collect::<Result<Vec<_>, Box<PluginError>>>()?;
        let plugin: Arc<dyn Plugin> = plugin;
        Ok(PluginInstance { plugin, handlers })
    }
}

pub(crate) fn supported_hook_name(hook: &str) -> Option<&'static str> {
    Operation::ALL.into_iter().flat_map(Operation::hooks).find(|name| *name == hook)
}
