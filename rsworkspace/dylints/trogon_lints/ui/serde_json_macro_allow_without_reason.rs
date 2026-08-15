// edition:2024

// A bare suppression records nothing and must fire.
#[allow(serde_json_macro)]
fn undocumented() {}

// `expect` suppresses just as effectively, so it owes the same reason.
#[expect(serde_json_macro)]
fn undocumented_expectation() {}

// An empty reason is not a justification.
#[allow(serde_json_macro, reason = "")]
fn blank_reason() {}

// The documented form: the exception is argued at the site.
#[allow(
    serde_json_macro,
    reason = "the upstream document is forwarded verbatim and has no fixed schema"
)]
fn justified() {}

// Suppressing a different lint says nothing about this one.
#[allow(function_local_use)]
fn unrelated_suppression() {}

fn main() {
    undocumented();
    undocumented_expectation();
    blank_reason();
    justified();
    unrelated_suppression();
}
