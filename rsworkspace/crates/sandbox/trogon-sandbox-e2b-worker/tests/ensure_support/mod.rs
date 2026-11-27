// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! Error plumbing for the fixtures: a boxed error and a way to name what was missing.

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;

/// Fails with `message` unless `condition` holds; for fixtures whose cleanup must run after a failure.
pub fn check(condition: bool, message: &str) -> Result<(), Boxed> {
    condition.then_some(()).ok_or_else(|| message.into())
}

/// Adds a message to a missing value or an error.
pub trait Context<T> {
    fn context(self, message: &str) -> Result<T, Boxed>;
    fn with_context<F: FnOnce() -> String>(self, message: F) -> Result<T, Boxed>;
}

impl<T> Context<T> for Option<T> {
    fn context(self, message: &str) -> Result<T, Boxed> {
        self.ok_or_else(|| message.into())
    }

    fn with_context<F: FnOnce() -> String>(self, message: F) -> Result<T, Boxed> {
        self.ok_or_else(|| message().into())
    }
}

impl<T, E: Into<Boxed>> Context<T> for Result<T, E> {
    fn context(self, message: &str) -> Result<T, Boxed> {
        self.map_err(|error| format!("{message}: {}", error.into()).into())
    }

    fn with_context<F: FnOnce() -> String>(self, message: F) -> Result<T, Boxed> {
        self.map_err(|error| format!("{}: {}", message(), error.into()).into())
    }
}
