use std::fmt;

use opentelemetry::metrics::{Counter, Meter};
use opentelemetry::{KeyValue, global};

use trogon_semconv::{attribute, metric};

use crate::constants::METER_NAME;
use crate::credential_source::CredentialSource;

/// The proxy's counters. Each accepted connection carries one CONNECT, so the
/// connection count is also the request count.
#[derive(Clone)]
pub(crate) struct ConnectionMetrics {
    accepted: Counter<u64>,
}

impl ConnectionMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            accepted: metric::build_egress_proxy_connections_accepted(meter),
        }
    }

    pub(crate) fn global() -> Self {
        Self::new(&global::meter(METER_NAME))
    }

    pub(crate) fn record_accepted(&self, source: CredentialSource) {
        self.accepted.add(
            1,
            &[KeyValue::new(
                attribute::EGRESS_PROXY_CREDENTIAL_SOURCE,
                source.as_str(),
            )],
        );
    }
}

impl fmt::Debug for ConnectionMetrics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConnectionMetrics { .. }")
    }
}

#[cfg(test)]
mod tests;
