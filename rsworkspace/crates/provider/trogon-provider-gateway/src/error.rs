#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    CryptoProvider(#[from] trogon_std::tls::CryptoProviderAlreadyInstalledError),
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
    #[error(transparent)]
    Server(#[from] crate::server::ServerError),
    #[error(transparent)]
    TelemetryShutdown(#[from] trogon_telemetry::TelemetryShutdownError),
}
