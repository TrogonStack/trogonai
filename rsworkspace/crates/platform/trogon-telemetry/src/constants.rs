use std::time::Duration;

pub use trogon_semconv::attribute::{
    ACP_PREFIX as RESOURCE_ATTRIBUTE_ACP_PREFIX, MCP_PREFIX as RESOURCE_ATTRIBUTE_MCP_PREFIX,
};

pub const METRIC_EXPORT_INTERVAL: Duration = Duration::from_secs(30);

/// Upper bound on [`crate::shutdown_otel`] when the collector is unreachable.
/// The SDK defaults to five seconds per provider, run one after the other, and
/// the meter provider ignores a caller-supplied timeout, so an unreachable
/// collector otherwise holds a process open for about fifteen seconds.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
