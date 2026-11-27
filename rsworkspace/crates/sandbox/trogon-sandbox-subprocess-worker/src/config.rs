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

impl FuseConfig {
    pub fn from_env(env: &impl ReadEnv) -> Self {
        let flag = |key: &str| env.var(key).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let enabled = flag("SANDBOX_FUSE_ENABLED");
        let mount_s3_binary = env
            .var("SANDBOX_FUSE_MOUNT_S3_BINARY")
            .map_or_else(|_| PathBuf::from("mount-s3"), PathBuf::from);
        let region = env.var("SANDBOX_FUSE_REGION").unwrap_or_default();
        let endpoint = env.var("SANDBOX_FUSE_ENDPOINT_URL").ok();
        let allow_other = flag("SANDBOX_FUSE_ALLOW_OTHER");
        Self {
            enabled,
            mount_s3_binary,
            region,
            endpoint,
            allow_other,
        }
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
