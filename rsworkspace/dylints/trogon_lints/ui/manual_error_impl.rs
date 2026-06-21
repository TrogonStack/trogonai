// edition:2024

use std::fmt;

#[derive(Debug)]
struct ManualError;

impl fmt::Display for ManualError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("manual error")
    }
}

// Hand-written `Error` impl: this is the violation.
impl std::error::Error for ManualError {}

#[derive(Debug)]
struct ManualWithSource(std::io::Error);

impl fmt::Display for ManualWithSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("manual error with source")
    }
}

// Hand-written `Error` impl with a body: also a violation.
impl std::error::Error for ManualWithSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

// A macro-generated `Error` impl stands in for a derive (e.g. thiserror): the
// generated impl comes from a macro expansion and must NOT be linted.
macro_rules! derive_error {
    ($ty:ty) => {
        impl std::error::Error for $ty {}
    };
}

#[derive(Debug)]
struct MacroError;

impl fmt::Display for MacroError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("macro error")
    }
}

derive_error!(MacroError);

// Unrelated trait impls must NOT be linted.
struct NotAnError;

impl fmt::Display for NotAnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not an error")
    }
}

fn main() {}
