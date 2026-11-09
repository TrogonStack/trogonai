//! Configuration values that may carry a `secret:` marker and a `base64:` encoding.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod constants;

mod config_value;
mod secret_marker;

pub use config_value::{ConfigValue, ConfigValueError};
pub use secret_marker::{SecretMarker, SecretMarkerError};
