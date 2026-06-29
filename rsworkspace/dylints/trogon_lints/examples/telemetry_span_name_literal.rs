//! UI fixture for the `telemetry_span_name_literal` lint. Built as an example
//! so the fixture can depend on the real `tracing` crate, whose span macros and
//! `#[instrument]` attribute the lint inspects.

use tracing::instrument;

const HTTP_SERVER_REQUEST: &str = "http.server.request";

fn inline_span() {
    let _span = tracing::info_span!("http.server.request", method = "GET");
}

fn constant_span() {
    let _span = tracing::info_span!(HTTP_SERVER_REQUEST, method = "GET");
}

fn inline_span_with_target() {
    let _span = tracing::info_span!(target: "fixture", "http.server.request");
}

#[instrument(name = "http.server.request")]
fn inline_instrument() {}

#[instrument(name = HTTP_SERVER_REQUEST)]
fn constant_instrument() {}

#[instrument]
fn plain_instrument() {}

fn main() {
    inline_span();
    constant_span();
    inline_span_with_target();
    inline_instrument();
    constant_instrument();
    plain_instrument();
}
