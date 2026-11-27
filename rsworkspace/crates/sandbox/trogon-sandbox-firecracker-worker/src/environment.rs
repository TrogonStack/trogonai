//! The worker's execution environment identity: what it hashes, and the checks built on it.

use std::path::Path;

use anyhow::{Context, bail};
use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::{EnvDigest, EnvironmentDefinition, LegacySnapshotPolicy, ProviderKind};
use trogon_sandbox_protocol::{EnvironmentError, EnvironmentPolicy, RestoreVerdict, environment_digest};
use trogon_std::env::ReadEnv;

use crate::config::FirecrackerConfig;

mod constants;
use constants::VERSION_TIMEOUT;

/// How the VMM is confined, as it enters the digest.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Isolation {
    Direct,
    Jailer,
}

impl Isolation {
    fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Jailer => "jailer",
        }
    }
}

/// `sha256:<hex>` of a file's bytes, read in a blocking task.
pub async fn file_digest(path: &Path) -> anyhow::Result<String> {
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let mut file = std::fs::File::open(&owned)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = std::io::Read::read(&mut file, &mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let mut out = String::from("sha256:");
        for byte in hasher.finalize() {
            out.push_str(&format!("{byte:02x}"));
        }
        Ok(out)
    })
    .await?
    .with_context(|| format!("hashing {}", path.display()))
}

/// The version token from `firecracker --version` output (`Firecracker v1.9.0`).
pub fn parse_vmm_version(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let version = line.strip_prefix("Firecracker")?.trim();
    (!version.is_empty()).then(|| version.to_string())
}

async fn vmm_version(binary: &Path) -> anyhow::Result<String> {
    let output = tokio::time::timeout(
        VERSION_TIMEOUT,
        tokio::process::Command::new(binary)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .context("firecracker --version timed out")?
    .with_context(|| format!("running {} --version", binary.display()))?;
    if !output.status.success() {
        bail!("{} --version failed", binary.display());
    }
    parse_vmm_version(&String::from_utf8_lossy(&output.stdout)).context("unrecognized firecracker --version output")
}

/// The executable a command name or path refers to: a path is used as given, a bare name is
/// searched for on `PATH`, as the launcher will.
pub fn resolve_executable(binary: &Path, env: &impl ReadEnv) -> std::path::PathBuf {
    if binary.components().count() > 1 {
        return binary.to_path_buf();
    }
    env.var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| binary.to_path_buf())
}

/// Build the definition from what the worker will actually run.
pub async fn definition(
    config: &FirecrackerConfig,
    jailer: Option<&Path>,
    env: &impl ReadEnv,
) -> anyhow::Result<EnvironmentDefinition> {
    let mut definition = EnvironmentDefinition::new(ProviderKind::Firecracker, std::env::consts::ARCH)
        .with_component("vmm_version", vmm_version(config.binary.as_path()).await?)
        .with_component(
            "vmm_binary",
            file_digest(&resolve_executable(config.binary.as_path(), env)).await?,
        )
        .with_component("kernel", file_digest(config.kernel.as_path()).await?);
    if let Some(jailer) = jailer {
        definition = definition.with_component("jailer_binary", file_digest(&resolve_executable(jailer, env)).await?);
    }
    definition.boot_args = config.boot_args.clone();
    definition.snapshot_format = crate::snapshot_files::FORMAT_VERSION;
    definition.isolation_profile = if jailer.is_some() {
        Isolation::Jailer
    } else {
        Isolation::Direct
    }
    .label()
    .to_string();
    definition.cpu_template = config.cpu_template.clone();
    Ok(definition)
}

/// Compute this worker's digest and install the node policy, refusing to continue when the
/// digest is not approved.
pub async fn install(
    config: &mut FirecrackerConfig,
    jailer: Option<&Path>,
    approved: Vec<EnvDigest>,
    legacy: LegacySnapshotPolicy,
    env: &impl ReadEnv,
) -> anyhow::Result<EnvDigest> {
    let digest = environment_digest(&definition(config, jailer, env).await?)?;
    config.environment = EnvironmentPolicy::new(digest.clone())
        .with_approved(approved)
        .with_legacy_snapshots(legacy);
    config.environment.check_startup()?;
    Ok(digest)
}

/// Apply the restore check to a snapshot's recorded digest, warning for an unlabeled one.
pub fn check_restore(policy: &EnvironmentPolicy, snapshot: Option<&EnvDigest>) -> Result<(), EnvironmentError> {
    if policy.check_restore(snapshot)? == RestoreVerdict::LegacyUnlabeled {
        tracing::warn!(
            "restoring a snapshot that records no environment digest; \
             set the legacy policy to refuse once old snapshots have aged out"
        );
    }
    Ok(())
}

/// What a restore checks against the worker's execution environment.
pub struct RestoreEnvironment<'a> {
    pub policy: &'a trogon_sandbox_protocol::EnvironmentPolicy,
    /// The digest the artifact's manifest annotation carries, when it has one.
    pub manifest_label: Option<trogon_sandbox_contract::EnvDigest>,
}

#[cfg(test)]
mod tests;
