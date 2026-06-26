//! Runtime entry point.
//!
//! Slice g2 ships the boot orchestration seam (`run_with_args`) and the
//! [`RuntimeError`] surface so [`crate::run`] and integration tests share a
//! single call path. The full ingress / audit / policy execution is filled
//! in by later slices (g3+); for now `run_with_args` walks the config seam
//! and returns once the bootstrap-only setup is complete.

use trogon_std::env::ReadEnv;

use crate::config::{Args, ConfigError, config_from_args};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// Config resolution failed at boot — the binary cannot proceed.
    #[error("gateway config: {0}")]
    Config(#[from] ConfigError),
}

/// Resolve config from `args` + `env` and run the gateway. The current slice
/// only exercises the config seam; later slices wire NATS subscription,
/// audit emission, and policy execution behind this same entry point so the
/// public surface stays stable across the series.
pub async fn run_with_args<E: ReadEnv>(args: Args, env: &E) -> Result<(), RuntimeError> {
    let (_config, _nats_config) = config_from_args(args, env)?;
    Ok(())
}

#[cfg(test)]
mod tests;
