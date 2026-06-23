//! Trace-context propagation from the consumed schedule event to the execution
//! schedule publish.
//!
//! `StreamEvent.event.headers` carries the trace context recorded when the
//! command produced the event. The processor extracts that context and injects
//! it into the execution schedule publish headers so the scheduled message is
//! causally linked to the originating command rather than orphaned. Trace
//! context is metadata: it never collides with a reserved scheduler header and
//! never affects ordering or the replay gate.

use async_nats::HeaderMap;
use opentelemetry::Context;
use opentelemetry::propagation::{Extractor, Injector};
use trogon_decider_runtime::Headers;

struct HeaderExtractor<'a>(&'a Headers);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get_str(key)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.iter().map(|(name, _)| name.as_str()).collect()
    }
}

struct HeaderInjector<'a>(&'a mut HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key, value.as_str());
    }
}

/// Extracts the OpenTelemetry context recorded on a consumed event's headers.
pub fn extract_context(headers: &Headers) -> Context {
    opentelemetry::global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)))
}

/// Builds the execution-publish headers carrying the propagated trace context.
pub fn execution_trace_headers(headers: &Headers) -> HeaderMap {
    let context = extract_context(headers);
    let mut out = HeaderMap::new();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(&mut out));
    });
    out
}

#[cfg(test)]
mod tests;
