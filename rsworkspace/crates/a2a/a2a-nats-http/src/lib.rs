#![doc = include_str!("../README.md")]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod handlers;
pub mod headers;
pub mod rest;
pub mod router;
pub mod runtime;
pub mod sse;

pub use runtime::{RuntimeError, run};

#[cfg(test)]
mod tests;
