/// A broker metric family could not be created or registered.
#[derive(Debug, thiserror::Error)]
#[error("broker metrics could not be registered: {0}")]
pub struct BrokerMetricsError(#[from] prometheus::Error);
