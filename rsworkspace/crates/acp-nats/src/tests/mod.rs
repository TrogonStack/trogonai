mod agent_handlers;
mod builders;
mod client_handlers;
mod prefix;
mod workflows;

pub use builders::*;
pub(crate) fn noop_meter() -> opentelemetry::metrics::Meter {
    opentelemetry::global::meter("acp-nats-test")
}
pub use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
