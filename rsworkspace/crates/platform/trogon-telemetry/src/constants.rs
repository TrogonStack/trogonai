use std::time::Duration;

pub use trogon_semconv::attribute::{
    ACP_PREFIX as RESOURCE_ATTRIBUTE_ACP_PREFIX, MCP_PREFIX as RESOURCE_ATTRIBUTE_MCP_PREFIX,
};

pub const METRIC_EXPORT_INTERVAL: Duration = Duration::from_secs(30);
