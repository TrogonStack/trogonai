use opentelemetry::propagation::TextMapPropagator;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use trogon_decider_runtime::HeaderName;

use super::*;

fn incoming(traceparent: &str) -> Headers {
    Headers::one(HeaderName::new("traceparent").unwrap(), traceparent).unwrap()
}

#[test]
fn trace_context_is_propagated_onto_execution_headers() {
    // Install a W3C propagator for the duration of the test rather than
    // relying on global process configuration.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let headers = incoming(traceparent);

    let out = execution_trace_headers(&headers);
    let propagated = out.get("traceparent").expect("traceparent propagated");

    // The trace id must be preserved so the scheduled message stays linked
    // to the originating command's trace.
    assert!(propagated.as_str().contains("4bf92f3577b34da6a3ce929d0e0e4736"));
}

#[test]
fn extractor_exposes_header_keys_and_values() {
    let propagator = TraceContextPropagator::new();
    let headers = incoming("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01");
    let extractor = HeaderExtractor(&headers);
    assert_eq!(extractor.keys(), vec!["traceparent"]);
    let context = propagator.extract(&extractor);
    // A valid remote span context round-trips back out.
    let mut out = HeaderMap::new();
    propagator.inject_context(&context, &mut HeaderInjector(&mut out));
    assert!(out.get("traceparent").is_some());
}
