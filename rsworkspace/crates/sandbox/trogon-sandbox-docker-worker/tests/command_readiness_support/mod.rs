// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::query_parameters::DownloadFromContainerOptionsBuilder;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use trogon_sandbox_contract::{DispatchTarget, LeaseId, ProviderKind, SandboxEntityId, SandboxUserName, TenantId};

const SUPERVISOR_LIMIT: usize = 64 * 1024 * 1024;

/// The static command supervisor the operator configured, and the digest of its bytes.
pub struct Supervisor {
    pub path: PathBuf,
    pub sha256: String,
}

impl Supervisor {
    /// Reads `SANDBOX_DOCKER_COMMAND_SUPERVISOR` and digests the artifact it names.
    pub fn configured() -> anyhow::Result<Self> {
        let path = PathBuf::from(
            trogon_std::env::ReadEnv::var_os(&trogon_std::env::SystemEnv, "SANDBOX_DOCKER_COMMAND_SUPERVISOR")
                .context("configure the sealed supervisor artifact in SANDBOX_DOCKER_COMMAND_SUPERVISOR")?,
        );
        let bytes = std::fs::read(&path)?;
        ensure!(
            !bytes.is_empty() && bytes.len() <= SUPERVISOR_LIMIT,
            "configured supervisor artifact has no bounded executable bytes"
        );
        Ok(Self {
            sha256: hex_digest(&bytes),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    pub provider: ProviderKind,
    pub target: DispatchTarget,
    pub sandbox_id: String,
    pub run_as_user: Option<SandboxUserName>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub sha256: String,
    pub protocol: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    IncompatibleIsolation,
    RuntimeNotConfigured,
    LegacyAssignment,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    RuntimeUnavailable,
    ProbeFailed,
    CustodyUnavailable,
    AuthorityUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeniedReason {
    InvalidBinding,
    ForeignBinding,
    UnknownBinding,
    InvalidRequest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Ready {
        binding: Binding,
        runtime: Runtime,
    },
    Unsupported {
        binding: Binding,
        reason: UnsupportedReason,
    },
    Unavailable {
        binding: Binding,
        reason: UnavailableReason,
    },
    Denied {
        reason: DeniedReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
pub enum ExistingScopeRefusalReason {
    #[serde(rename = "lease id is already held for a different tenant")]
    LeaseTenantConflict,
    #[serde(rename = "request is for an attempt that is not the live one for this lease")]
    StaleAttempt,
}

#[derive(Deserialize)]
#[serde(tag = "error", rename_all = "snake_case", deny_unknown_fields)]
enum ExistingScopeRefusal {
    DispatchAuth { message: ExistingScopeRefusalReason },
}

pub fn assert_existing_scope_refusal(payload: &[u8], expected: ExistingScopeRefusalReason) -> anyhow::Result<()> {
    ensure!(
        payload.len() <= 256,
        "existing verifier refusal exceeded its bounded contract"
    );
    let refusal: ExistingScopeRefusal = serde_json::from_slice(payload)
        .map_err(|_| anyhow::anyhow!("existing verifier refusal violated its strict typed contract"))?;
    let ExistingScopeRefusal::DispatchAuth { message } = refusal;
    ensure!(
        message == expected,
        "existing verifier refused the scope for an unexpected reason"
    );
    Ok(())
}

pub fn normalize(capability: &str) -> String {
    capability
        .trim()
        .to_ascii_uppercase()
        .trim_start_matches("CAP_")
        .to_owned()
}

fn is_fresh_id(value: &str) -> bool {
    SandboxEntityId::new(value).is_ok()
}

/// Proves `native` mounts the configured supervisor read-only from an owned, labelled, sealed volume, and returns
/// that volume's name.
pub async fn sealed_runtime(docker: &Docker, native: &str, owner: &str, digest: &str) -> anyhow::Result<String> {
    let found = docker.inspect_container(native, None).await?;
    ensure!(
        found.id.as_deref() == Some(native),
        "runtime container identity changed"
    );
    let prefix = format!("sandbox-command-{digest}-");
    let mount = found
        .mounts
        .as_ref()
        .and_then(|mounts| {
            mounts.iter().find(|mount| {
                mount.typ.as_ref().map(ToString::to_string).as_deref() == Some("volume")
                    && mount.rw == Some(false)
                    && mount.name.as_ref().is_some_and(|name| name.starts_with(&prefix))
            })
        })
        .context("actual native lease lacks its immutable supervisor mount")?;
    let volume = mount.name.as_ref().context("runtime volume identity is absent")?;
    let root = mount.destination.as_ref().context("runtime destination is absent")?;
    ensure!(
        root.strip_prefix("/.trogon-sandbox-run-").is_some_and(is_fresh_id),
        "runtime mount is not an unpredictable direct root mount"
    );
    let inspected = docker.inspect_volume(volume).await?;
    let nonce = inspected
        .labels
        .get("sandbox.trogon.live/command-runtime-registration")
        .context("runtime volume registration is absent")?;
    ensure!(
        inspected.name == *volume
            && inspected.driver == "local"
            && inspected.options.is_empty()
            && inspected.labels.get("sandbox.trogon.live/owner").map(String::as_str) == Some(owner)
            && inspected
                .labels
                .get("sandbox.trogon.live/command-runtime-digest")
                .map(String::as_str)
                == Some(digest)
            && is_fresh_id(nonce)
            && volume == &format!("sandbox-command-{digest}-{nonce}"),
        "runtime volume provenance changed"
    );
    let mut stream = docker.download_from_container(
        native,
        Some(
            DownloadFromContainerOptionsBuilder::new()
                .path(&format!("{root}/supervisor"))
                .build(),
        ),
    );
    let mut archive = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        ensure!(
            archive.len().saturating_add(chunk.len()) <= SUPERVISOR_LIMIT + 65536,
            "supervisor archive exceeds its bounded artifact size"
        );
        archive.extend_from_slice(&chunk);
    }
    let mut archive = tar::Archive::new(Cursor::new(archive));
    let mut files = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        ensure!(
            entry.header().entry_type().is_file()
                && entry.path()?.as_ref() == Path::new("supervisor")
                && entry.header().uid()? == 0
                && entry.header().gid()? == 0
                && entry.header().mode()? & 0o777 == 0o555,
            "sealed supervisor metadata changed"
        );
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(SUPERVISOR_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= SUPERVISOR_LIMIT, "supervisor bytes exceed their bound");
        ensure!(
            hex_digest(&bytes) == digest,
            "mounted helper differs from the configured artifact bytes"
        );
        files += 1;
    }
    ensure!(files == 1, "supervisor archive must contain exactly its executable");
    Ok(volume.clone())
}

pub fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
