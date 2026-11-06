#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod unix_endpoint;

pub use unix_endpoint::{UnixEndpoint, UnixEndpointError};
