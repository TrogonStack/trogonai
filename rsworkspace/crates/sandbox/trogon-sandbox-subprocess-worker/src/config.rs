use std::path::PathBuf;

use trogon_std::env::ReadEnv;

pub use crate::backend_kind::IsolationBackendKind;

#[derive(Debug, Clone)]
pub struct FuseConfig {
    pub enabled: bool,
    pub mount_s3_binary: PathBuf,
    pub region: String,
    pub endpoint: Option<String>,
    pub allow_other: bool,
}

/// A `SANDBOX_FUSE_*` variable the worker refuses to start with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FuseConfigError {
    #[error("{name} must be one of 1, true, 0 or false")]
    NotAFlag { name: &'static str },
    #[error("{name} is not valid unicode")]
    NotUnicode { name: &'static str },
}

fn optional(env: &impl ReadEnv, name: &'static str) -> Result<Option<String>, FuseConfigError> {
    match env.var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(FuseConfigError::NotUnicode { name }),
    }
}

fn flag(env: &impl ReadEnv, name: &'static str) -> Result<bool, FuseConfigError> {
    match optional(env, name)? {
        None => Ok(false),
        Some(value) if value == "1" || value.eq_ignore_ascii_case("true") => Ok(true),
        Some(value) if value == "0" || value.eq_ignore_ascii_case("false") => Ok(false),
        Some(_) => Err(FuseConfigError::NotAFlag { name }),
    }
}

impl FuseConfig {
    pub fn from_env(env: &impl ReadEnv) -> Result<Self, FuseConfigError> {
        let enabled = flag(env, "SANDBOX_FUSE_ENABLED")?;
        let mount_s3_binary =
            optional(env, "SANDBOX_FUSE_MOUNT_S3_BINARY")?.map_or_else(|| PathBuf::from("mount-s3"), PathBuf::from);
        let region = optional(env, "SANDBOX_FUSE_REGION")?.unwrap_or_default();
        let endpoint = optional(env, "SANDBOX_FUSE_ENDPOINT_URL")?;
        let allow_other = flag(env, "SANDBOX_FUSE_ALLOW_OTHER")?;
        Ok(Self {
            enabled,
            mount_s3_binary,
            region,
            endpoint,
            allow_other,
        })
    }
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub agent_binary_default: PathBuf,
    pub workspace_root: PathBuf,
    pub isolation_backend: IsolationBackendKind,
    pub bwrap_binary_path: Option<PathBuf>,
    pub egress_proxy_binary_path: Option<PathBuf>,
    pub relay_binary_path: Option<PathBuf>,
    pub fuse: FuseConfig,
}

impl WorkerConfig {
    #[cfg(target_os = "linux")]
    pub fn resolve_bwrap_path(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = &self.bwrap_binary_path {
            return Ok(path.clone());
        }
        which::which("bwrap").map_err(|_| {
            anyhow::anyhow!(
                "bubblewrap (`bwrap`) not found on PATH; install the `bubblewrap` package or set --bwrap-binary-path"
            )
        })
    }

    pub fn resolve_egress_proxy_path(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = &self.egress_proxy_binary_path {
            return Ok(path.clone());
        }
        which::which("trogon-sandbox-egress-proxyd").map_err(|_| {
            anyhow::anyhow!(
                "`trogon-sandbox-egress-proxyd` not found on PATH; install it or set --egress-proxy-binary-path"
            )
        })
    }
}

#[cfg(test)]
mod tests;
