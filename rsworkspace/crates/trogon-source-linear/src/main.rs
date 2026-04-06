#[cfg(not(coverage))]
use {
    acp_telemetry::ServiceName, tracing::error, tracing::info, trogon_nats::connect,
    trogon_nats::jetstream::ClaimCheckPublisher, trogon_nats::jetstream::MaxPayload,
    trogon_nats::jetstream::NatsJetStreamClient, trogon_nats::jetstream::NatsObjectStore,
    trogon_source_linear::LinearConfig,
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
    let max_payload = MaxPayload::from_server_limit(nats.server_info().max_payload);
    let js_context = async_nats::jetstream::new(nats);
    let object_store = NatsObjectStore::provision(
        &js_context,
        async_nats::jetstream::object_store::Config {
            bucket: "trogon-claims".to_string(),
            ..Default::default()
        },
    )
    .await?;
    let client = NatsJetStreamClient::new(js_context);
    let publisher = ClaimCheckPublisher::new(
        client.clone(),
        object_store,
        "trogon-claims".to_string(),
        max_payload,
    );
    let result = trogon_source_linear::serve(client, publisher, config).await;

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
