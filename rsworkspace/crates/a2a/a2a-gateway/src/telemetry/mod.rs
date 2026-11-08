use opentelemetry::{KeyValue, metrics::Counter};

pub(crate) struct RelayAdmissionMetrics {
    refusals: Counter<u64>,
}

impl RelayAdmissionMetrics {
    pub(crate) fn new(meter: &opentelemetry::metrics::Meter) -> Self {
        Self {
            refusals: trogon_semconv::metric::build_a2a_gateway_imported_reply_capacity_refusals(meter),
        }
    }

    pub(crate) fn global_refusal(&self) {
        self.refusals.add(
            1,
            &[KeyValue::new(
                trogon_semconv::attribute::A2A_GATEWAY_RELAY_LIMIT,
                "global",
            )],
        );
    }

    pub(crate) fn caller_refusal(&self) {
        self.refusals.add(
            1,
            &[KeyValue::new(
                trogon_semconv::attribute::A2A_GATEWAY_RELAY_LIMIT,
                "caller",
            )],
        );
    }
}

#[cfg(test)]
mod tests;
