//! Wraps a failure in what was being attempted.

use std::fmt::Display;

use crate::error::FenceHelperError;

pub trait Context<T> {
    fn context<C: Display>(self, context: C) -> Result<T, FenceHelperError>;

    fn with_context<C: Display, F: FnOnce() -> C>(self, context: F) -> Result<T, FenceHelperError>;
}

fn wrap<C: Display>(context: C, source: FenceHelperError) -> FenceHelperError {
    FenceHelperError::Context {
        context: context.to_string(),
        source: Box::new(source),
    }
}

impl<T, E: Into<FenceHelperError>> Context<T> for Result<T, E> {
    fn context<C: Display>(self, context: C) -> Result<T, FenceHelperError> {
        self.map_err(|e| wrap(context, e.into()))
    }

    fn with_context<C: Display, F: FnOnce() -> C>(self, context: F) -> Result<T, FenceHelperError> {
        self.map_err(|e| wrap(context(), e.into()))
    }
}

impl<T> Context<T> for Option<T> {
    fn context<C: Display>(self, context: C) -> Result<T, FenceHelperError> {
        self.ok_or_else(|| FenceHelperError::msg(context.to_string()))
    }

    fn with_context<C: Display, F: FnOnce() -> C>(self, context: F) -> Result<T, FenceHelperError> {
        self.ok_or_else(|| FenceHelperError::msg(context().to_string()))
    }
}
