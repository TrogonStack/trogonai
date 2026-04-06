#[cfg(not(coverage))]
use {
    tracing::error, tracing::info, trogon_nats::connect,
    trogon_telemetry::ServiceName,
    trogon_nats::jetstream::NatsJetStreamClient, trogon_source_telegram::TelegramSourceConfig,
    trogon_source_telegram::constants::DEFAULT_NATS_CONNECT_TIMEOUT, trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = TelegramSourceConfig::from_env(&SystemEnv);

    trogon_telemetry::init_logger(
        ServiceName::TrogonSourceTelegram,
        [],
        &SystemEnv,
        &SystemFs,
    );

    info!("Telegram webhook source starting");

    let nats = connect(&config.nats, DEFAULT_NATS_CONNECT_TIMEOUT).await?;
    let js = NatsJetStreamClient::new(async_nats::jetstream::new(nats));
    let result = trogon_source_telegram::serve(js, config).await;

    if let Err(ref e) = result {
        error!(error = %e, "Telegram webhook source stopped with error");
    } else {
        info!("Telegram webhook source stopped");
    }

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

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
