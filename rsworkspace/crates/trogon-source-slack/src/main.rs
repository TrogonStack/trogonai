#[cfg(not(coverage))]
use {
    acp_telemetry::ServiceName, tracing::error, tracing::info, trogon_nats::connect,
    trogon_nats::jetstream::NatsJetStreamClient, trogon_source_slack::SlackConfig,
    trogon_source_slack::constants::DEFAULT_NATS_CONNECT_TIMEOUT, trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = SlackConfig::from_env(&SystemEnv);

    acp_telemetry::init_logger(
        ServiceName::TrogonSourceSlack,
        &config.subject_prefix,
        &SystemEnv,
        &SystemFs,
    );

    info!("Slack webhook server starting");

    let nats = connect(&config.nats, DEFAULT_NATS_CONNECT_TIMEOUT).await?;
    let js = NatsJetStreamClient::new(async_nats::jetstream::new(nats));
    let result = trogon_source_slack::serve(js, config).await;

    if let Err(ref e) = result {
        error!(error = %e, "Slack webhook server stopped with error");
    } else {
        info!("Slack webhook server stopped");
    }

    acp_telemetry::shutdown_otel();

    result.map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
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
