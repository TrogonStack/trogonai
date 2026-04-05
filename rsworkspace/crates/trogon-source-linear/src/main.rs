#[cfg(not(coverage))]
use {
    acp_telemetry::ServiceName, tracing::error, tracing::info, trogon_nats::connect,
    trogon_nats::jetstream::NatsJetStreamClient, trogon_source_linear::LinearConfig,
    trogon_source_linear::constants::DEFAULT_NATS_CONNECT_TIMEOUT, trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = LinearConfig::from_env(&SystemEnv);

    acp_telemetry::init_logger(
        ServiceName::TrogonSourceLinear,
        &config.subject_prefix,
        &SystemEnv,
        &SystemFs,
    );

    info!("Linear webhook server starting");

    let nats = connect(&config.nats, DEFAULT_NATS_CONNECT_TIMEOUT).await?;
    let js = NatsJetStreamClient::new(async_nats::jetstream::new(nats));
    let result = trogon_source_linear::serve(js, config).await;

    if let Err(ref e) = result {
        error!(error = %e, "Linear webhook server stopped with error");
    } else {
        info!("Linear webhook server stopped");
    }

    acp_telemetry::shutdown_otel();

    result.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
}

#[cfg(coverage)]
fn main() {}

#[cfg(all(coverage, test))]
mod tests {
    #[test]
    fn coverage_stub() {
        super::main();
    }
}
