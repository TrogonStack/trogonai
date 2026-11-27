use std::fmt::Display;

pub(crate) type TestError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type TestResult<T = ()> = Result<T, TestError>;

/// Turns a missing value or a failure into a test error that says what was expected.
pub(crate) trait Context<T> {
    fn context(self, message: &str) -> TestResult<T>;
}

impl<T, E: Display> Context<T> for Result<T, E> {
    fn context(self, message: &str) -> TestResult<T> {
        self.map_err(|error| format!("{message}: {error}").into())
    }
}

impl<T> Context<T> for Option<T> {
    fn context(self, message: &str) -> TestResult<T> {
        self.ok_or_else(|| message.into())
    }
}
