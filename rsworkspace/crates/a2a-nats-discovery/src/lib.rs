#![doc = include_str!("../README.md")]

pub mod config;
pub mod runtime;

pub use config::{Args, ConfigError};
pub use runtime::{ProvisionCatalogError, RuntimeError};

use trogon_std::env::SystemEnv;

pub async fn run(args: Args) -> Result<(), RuntimeError> {
    runtime::run_with_args(args, &SystemEnv).await
}
