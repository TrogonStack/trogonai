use std::future::Future;
use std::pin::Pin;

use crate::{CheckFailureError, Target};

/// The future a check returns.
pub type CheckFuture<'a> = Pin<Box<dyn Future<Output = Result<(), CheckFailureError>> + Send + 'a>>;

/// One conformance check, run against a target.
pub type CheckFn = for<'a> fn(&'a Target) -> CheckFuture<'a>;
