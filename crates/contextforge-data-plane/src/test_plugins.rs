use contextforge_data_plane_cpex::{CpexRuntimeRegistry, GatewayPluginFactory};

pub fn register(runtime: &mut CpexRuntimeRegistry) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    runtime.register_factory(
        "contextforge/payload-marker",
        Box::new(GatewayPluginFactory::new(cpex_payload_marker::PayloadMarkerPlugin::new).with_cmf_hooks()),
    )?;
    runtime.register_factory(
        "contextforge/text-prefixer",
        Box::new(GatewayPluginFactory::new(cpex_text_prefixer::TextPrefixerPlugin::new).with_cmf_hooks()),
    )?;
    runtime.register_factory(
        "contextforge/tool-namespace",
        Box::new(GatewayPluginFactory::new(cpex_tool_namespace::ToolNamespacePlugin::new).with_cmf_hooks()),
    )?;
    Ok(())
}
