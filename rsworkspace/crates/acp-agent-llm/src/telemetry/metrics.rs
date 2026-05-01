use opentelemetry::metrics::{Counter, Histogram, Meter};

pub struct Metrics {
    pub llm_requests: Counter<u64>,
    pub llm_errors: Counter<u64>,
    pub llm_duration: Histogram<f64>,
    pub llm_tokens_in: Counter<u64>,
    pub llm_tokens_out: Counter<u64>,
}

impl Metrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            llm_requests: meter.u64_counter("llm.requests").build(),
            llm_errors: meter.u64_counter("llm.errors").build(),
            llm_duration: meter.f64_histogram("llm.duration").build(),
            llm_tokens_in: meter.u64_counter("llm.tokens.input").build(),
            llm_tokens_out: meter.u64_counter("llm.tokens.output").build(),
        }
    }
}
