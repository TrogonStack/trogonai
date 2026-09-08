//! UI fixture for the `unstructured_log_fields` lint. Built as an example so
//! the fixture can depend on the real `tracing` crate, whose event macros the
//! lint inspects.
//!
//! The example is also built by plain `cargo build`, where the lint this
//! fixture suppresses is not registered, so the file waives `unknown_lints`.
#![allow(unknown_lints)]

static FIXTURE_SPAN: &str = "fixture.span";

fn positional_arguments() {
    let user_id = 7;
    let path = "/v1/tasks";
    tracing::info!("user {} hit {}", user_id, path);
}

fn inline_capture() {
    let count = 3;
    tracing::warn!("processed {count} items");
}

fn debug_placeholder() {
    let request = "GET /";
    tracing::debug!("request: {request:?}");
}

fn targeted_event() {
    let subject = "acp.session";
    tracing::error!(target: "fixture", "publish to {} failed", subject);
}

fn structured_fields() {
    let user_id = 7;
    let path = "/v1/tasks";
    tracing::info!(user_id, path, "user hit endpoint");
}

fn debug_shorthand() {
    let request = "GET /";
    tracing::debug!(?request, "incoming request");
}

fn display_shorthand() {
    let address = "127.0.0.1:4222";
    tracing::info!(%address, "listening");
}

/// A field beside the message covers what it names, not the separate value the
/// message interpolates, so `action` is still a finding despite `user_id`.
fn mixed_field_and_message() {
    let user_id = 7;
    let action = "rename";
    tracing::info!(user_id, "user performed {}", action);
}

fn constant_message() {
    tracing::info!("server started");
}

fn escaped_braces() {
    tracing::info!("payload uses {{}} placeholders");
}

#[allow(unstructured_log_fields)]
fn allowed() {
    let user_id = 7;
    tracing::info!("user {}", user_id);
}

fn span_name() {
    let _span = tracing::info_span!(FIXTURE_SPAN, user_id = 7);
}

trait Constructor {
    fn new() -> Self;
}

struct Widget;

impl Constructor for Widget {
    fn new() -> Self {
        Widget
    }
}

/// A call that resolves to a trait's own associated fn named `new` has a trait,
/// not an `impl`, as its parent. The lint has to recognise that before asking
/// for a self type.
fn trait_associated_new<T: Constructor>() -> T {
    T::new()
}

/// Braces in a `target:` belong to the target, not to the message, so a
/// constant message beside one still captures nothing.
fn target_containing_braces() {
    tracing::error!(target: "audit{v2}", "server started");
}

/// The braces delimiting a macro invocation are not message placeholders
/// either.
fn brace_delimited_constant() {
    tracing::info! { "server started" }
}

/// A brace-delimited invocation that does interpolate is still a finding.
fn brace_delimited_interpolation() {
    let user_id = 7;
    tracing::info! { "user {} hit", user_id }
}

fn main() {
    positional_arguments();
    inline_capture();
    debug_placeholder();
    targeted_event();
    structured_fields();
    debug_shorthand();
    display_shorthand();
    mixed_field_and_message();
    constant_message();
    escaped_braces();
    allowed();
    span_name();
    let _widget: Widget = trait_associated_new();
    target_containing_braces();
    brace_delimited_constant();
    brace_delimited_interpolation();
}
