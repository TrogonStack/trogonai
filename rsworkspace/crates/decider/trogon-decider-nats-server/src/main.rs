#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use {
    anyhow::Result,
    async_nats::jetstream,
    std::sync::Arc,
    tracing::error,
    trogon_decider_nats_server::constants::NATS_CONNECT_TIMEOUT,
    trogon_decider_nats_server::{
        Args, DeciderHost, FileModuleSource, ModuleStore, ObjectStoreModuleSource, base_config, serve,
    },
    trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
    trogon_telemetry::ServiceName,
};

#[tokio::main]
async fn main() -> Result<()> {
    let (config, nats_config) = base_config(&trogon_std::CliArgs::<Args>::new(), &SystemEnv)?;
    trogon_telemetry::init_logger(ServiceName::TrogonDeciderNatsServer, [], &SystemEnv, &SystemFs);

    let client = trogon_nats::connect(&nats_config, NATS_CONNECT_TIMEOUT).await?;
    let js = jetstream::new(client.clone());

    // The only place in the binary that knows more than one module store
    // exists: past here the host is compiled against whichever one it got.
    let host = Arc::new(match &config.module_store {
        ModuleStore::Directory(root) => DeciderHost::start(&config, &FileModuleSource::new(root), js).await?,
        ModuleStore::ObjectStore(bucket) => {
            let source = ObjectStoreModuleSource::open(&js, bucket.clone()).await?;
            DeciderHost::start(&config, &source, js).await?
        }
    });

    let result = serve(host, client, &config).await;

    if let Err(error) = trogon_telemetry::shutdown_otel() {
        error!(error = %error, "OpenTelemetry shutdown failed");
    }

    result?;
    Ok(())
}
