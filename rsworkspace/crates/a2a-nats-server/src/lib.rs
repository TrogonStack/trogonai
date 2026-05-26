#![doc = include_str!("../README.md")]

pub mod handlers;
pub mod router;
pub mod runtime;
pub mod sse;

pub use runtime::{RuntimeError, run};

#[cfg(test)]
mod tests;
