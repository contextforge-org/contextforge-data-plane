#[cfg(feature = "test-plugins")]
mod test_plugins;

use std::sync::Arc;

use clap::Parser;

use contextforge_data_plane_cpex::CpexRuntimeRegistry;
use contextforge_data_plane_lib::{
    CliConfig, Config, ConfigStoreError, Gateway, RedisClient, UserConfigStoreType, get_authorization_service,
};
use contextforge_data_plane_observability::ObservabilityRuntime;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rustls::crypto;
use tikv_jemallocator::Jemalloc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider = crypto::aws_lc_rs::default_provider();
    _ = provider.install_default();

    let config = CliConfig::try_parse()?;
    let config = Config::try_from(config)?;

    let observability = ObservabilityRuntime::install(&config.observability_config)?;
    info!("starting contextforge-data-plane {config:?}");

    let config = match contextforge_data_plane_lib::get_global_config(&config.redis_config).await {
        Ok(global_config) => config.merge(global_config)?,
        Err(ConfigStoreError::NoDataForKey) => {
            info!("Starting without GlobalConfiguration");
            config
        },
        Err(e) => return Err(e.to_string().into()),
    };

    let plugin_registry = if config.runtime_plugins_enabled.unwrap_or(false) {
        Some(Arc::new(plugin_runtime_from_config(&config)?))
    } else {
        None
    };
    let plugin_runtime = plugin_registry.as_ref().map(|runtime| runtime.handle());

    let authorization_service = get_authorization_service(&config.jwks_config)?;

    let gateway = Gateway::builder()
        .with_config(config)
        .with_user_config_store_type(UserConfigStoreType::Redis)
        .with_session_manager(Arc::new(LocalSessionManager::default()))
        .with_plugin_runtime(plugin_runtime.clone())
        .with_authorization_service(authorization_service)
        .build();

    let _cpex_watcher = initialize_cpex_runtime(plugin_registry).await?;
    let gateway_result = run_gateway(gateway).await;
    if let Err(shutdown_error) = observability.shutdown() {
        error!("main - failed to shut down observability providers error = {shutdown_error}");
    }
    gateway_result
}

fn plugin_runtime_from_config(
    config: &Config,
) -> Result<CpexRuntimeRegistry, Box<dyn std::error::Error + Send + Sync>> {
    let redis_client = RedisClient::try_from(config.redis_config.clone())?;
    let plugin_runtime = CpexRuntimeRegistry::with_redis_config(redis_client);
    #[cfg(any(feature = "test-plugins", feature = "plugins"))]
    let plugin_runtime = register_builtin_factories(plugin_runtime)?;
    Ok(plugin_runtime)
}

#[cfg(any(feature = "test-plugins", feature = "plugins"))]
fn register_builtin_factories(
    mut plugin_runtime: CpexRuntimeRegistry,
) -> Result<CpexRuntimeRegistry, Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(feature = "test-plugins")]
    {
        test_plugins::register(&mut plugin_runtime)?;
    }
    #[cfg(feature = "plugins")]
    {
        plugin_runtime.register_factory(
            cpex_secrets_detection::KIND,
            Box::new(cpex_secrets_detection::SecretsDetectionFactory),
        )?;
    }
    Ok(plugin_runtime)
}

pub async fn initialize_cpex_runtime(
    cpex_runtime: Option<Arc<CpexRuntimeRegistry>>,
) -> contextforge_data_plane_lib::Result<Option<JoinHandle<()>>> {
    let Some(cpex_runtime) = cpex_runtime else {
        return Ok(None);
    };
    match cpex_runtime.initialize().await {
        Ok(Some(handle)) => {
            debug!("CPEX Plugins initialization successful");
            Ok(Some(handle))
        },
        Ok(None) => {
            debug!("CPEX Plugins initialization skipped");
            Ok(None)
        },
        Err(e) => {
            error!("CPEX Plugins initialization failed {e:?}");
            Err(e)
        },
    }
}

pub async fn run_gateway(gateway: Gateway) -> contextforge_data_plane_lib::Result<()> {
    let res = gateway.run_gateway().await;
    if res.is_ok() {
        debug!("Gateway process terminated");
    } else {
        error!("Gateway process terminated {res:?}");
    }
    Ok(())
}
