use std::sync::Arc;

use cpex::cpex_core::{
    cmf::CmfHook,
    error::PluginError,
    factory::{PluginFactory, PluginInstance},
    hooks::{HookHandler, TypedHandlerAdapter},
    plugin::{Plugin, PluginConfig},
    registry::AnyHookHandler,
};

pub struct CmfPluginFactory<P> {
    build: Box<dyn Fn(PluginConfig) -> P + Send + Sync>,
}

impl<P> CmfPluginFactory<P> {
    pub fn new(build: impl Fn(PluginConfig) -> P + Send + Sync + 'static) -> Self {
        Self { build: Box::new(build) }
    }
}

impl<P> PluginFactory for CmfPluginFactory<P>
where
    P: Plugin + HookHandler<CmfHook> + 'static,
{
    fn create(&self, config: &PluginConfig) -> Result<PluginInstance, Box<PluginError>> {
        let plugin = Arc::new((self.build)(config.clone()));
        let handlers = config
            .hooks
            .iter()
            .filter_map(|hook| supported_cmf_hook_name(hook))
            .map(|hook| {
                (hook, Arc::new(TypedHandlerAdapter::<CmfHook, _>::new(Arc::clone(&plugin))) as Arc<dyn AnyHookHandler>)
            })
            .collect::<Vec<_>>();

        if handlers.is_empty() {
            return Err(Box::new(PluginError::Config {
                message: format!("plugin '{}' does not declare supported CMF hooks", config.name),
            }));
        }

        let plugin: Arc<dyn Plugin> = plugin;
        Ok(PluginInstance { plugin, handlers })
    }
}

pub(crate) fn supported_cmf_hook_name(hook: &str) -> Option<&'static str> {
    crate::cmf::Operation::ALL.into_iter().flat_map(crate::cmf::Operation::hooks).find(|name| *name == hook)
}
