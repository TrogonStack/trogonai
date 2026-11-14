//! The text of a pydantic `ValidationError` (pydantic 2.14, pydantic-core 2.50): the `N validation errors for X`
//! header, then one block per error with its location, message, `[type=..., input_value=..., input_type=...]` and the
//! documentation link, with the input value spelled as Python `repr` and cut the way pydantic-core cuts it.

use crate::{
    constants,
    python_value_repr::{repr, type_name},
};
use serde_json::Value;

/// What an error reports as its `input_value`.
#[derive(Clone)]
pub(crate) enum ErrorInput {
    /// The value the validated model was given as a whole.
    Whole,
    Value(Value),
}

/// One entry of `ValidationError.errors()`.
#[derive(Clone)]
pub(crate) struct LineError {
    pub(crate) location: Vec<&'static str>,
    pub(crate) kind: &'static str,
    pub(crate) message: &'static str,
    pub(crate) input: ErrorInput,
}

/// `str(ValidationError)` for errors over `whole`, the value the validated type was given.
pub(crate) fn render(title: &str, errors: &[LineError], whole: &Value) -> String {
    let count = errors.len();
    let mut text = format!(
        "{count} validation error{} for {title}",
        if count == 1 { "" } else { "s" }
    );
    for error in errors {
        let input = match &error.input {
            ErrorInput::Whole => whole,
            ErrorInput::Value(value) => value,
        };
        text.push_str(&format!(
            "\n{}\n  {} [type={}, input_value={}, input_type={}]\n    For further information visit https://errors.pydantic.dev/{DOCUMENTATION_VERSION}/v/{}",
            error.location.join("."),
            error.message,
            error.kind,
            truncated(repr(input)),
            type_name(input),
            error.kind,
            DOCUMENTATION_VERSION = constants::PYDANTIC_DOCUMENTATION_VERSION,
        ));
    }
    text
}

/// A `repr` longer than 50 bytes keeps its first 25 bytes and last 24 bytes around `...`, each cut moved to a
/// character boundary.
fn truncated(text: String) -> String {
    if text.len() <= constants::PYDANTIC_MAX_REPR_BYTES {
        return text;
    }
    let mut head = constants::PYDANTIC_REPR_HEAD_BYTES;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - constants::PYDANTIC_REPR_TAIL_BYTES;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}...{}", &text[..head], &text[tail..])
}

#[cfg(test)]
#[path = "pydantic_validation_error_tests.rs"]
mod tests;
