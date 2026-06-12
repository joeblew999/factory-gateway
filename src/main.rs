//! factory-gateway binary — load a factory config, build its machines, and serve
//! them as a standard OPC-UA for Machinery address space.
//!
//! ```text
//! factory-gateway --config factory.toml
//! ```

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

use factory_gateway::{config::FactoryConfig, gateway::Gateway, http, opcua, registry::DriverRegistry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("factory_gateway=info".parse()?))
        .init();

    let config_path = std::env::args()
        .skip_while(|a| a != "--config")
        .nth(1)
        .unwrap_or_else(|| "factory.toml".to_owned());

    let config = FactoryConfig::load(std::path::Path::new(&config_path))?;
    let registry = DriverRegistry::with_builtin_drivers();

    tracing::info!(
        factory = %config.factory.name,
        machines = config.machines.len(),
        drivers = ?registry.supported_kinds(),
        "starting factory-gateway"
    );

    let gateway = Gateway::build(&config, &registry)?;
    let gateway = Arc::new(Mutex::new(gateway));

    // HTTP dashboard + job submit, alongside the OPC-UA server.
    tokio::spawn(http::run(config.http, gateway.clone()));
    opcua::serve(config.opcua, gateway).await
}
