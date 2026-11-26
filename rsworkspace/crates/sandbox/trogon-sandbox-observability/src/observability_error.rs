use std::net::SocketAddr;

/// Failure while setting up tracing or the metrics endpoint.
#[derive(Debug, thiserror::Error)]
pub enum ObservabilityError {
    #[error("failed to build the OTLP span exporter")]
    Exporter(#[source] opentelemetry_otlp::ExporterBuildError),
    #[error("prometheus registry operation failed")]
    Metrics(#[source] prometheus::Error),
    #[error("metrics listener on {addr} failed")]
    Listener {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}
