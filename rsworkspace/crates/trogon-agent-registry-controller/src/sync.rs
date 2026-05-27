use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tracing::info;

use crate::audit::{AuditEvent, AuditPublisher, classify_mutation, lifecycle_label};
use crate::kv::{ControllerStore, KvError};
use crate::manifest::{
    ManifestError, SignedManifest, ValidationError, monotonic_version, parse_manifest, validate_manifest_record,
    verify_manifest_signature,
};
use crate::signer::ManifestSigner;

#[derive(Debug, Clone)]
pub struct GitSyncConfig {
    pub repo_path: PathBuf,
    pub agents_dir: PathBuf,
    pub git_remote: Option<String>,
    pub git_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub git_commit: String,
    pub manifests_seen: usize,
    pub manifests_applied: usize,
    pub dry_run: bool,
}

#[derive(Debug)]
pub enum SyncError {
    Git(String),
    Io(std::io::Error),
    Manifest { path: PathBuf, error: ManifestError },
    Validation { path: PathBuf, error: ValidationError },
    Kv(KvError),
    Signer(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(message) => write!(f, "git error: {message}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Manifest { path, error } => write!(f, "manifest `{}`: {error}", path.display()),
            Self::Validation { path, error } => write!(f, "validation `{}`: {error}", path.display()),
            Self::Kv(error) => write!(f, "{error}"),
            Self::Signer(message) => write!(f, "signer error: {message}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Kv(error) => Some(error),
            _ => None,
        }
    }
}

pub struct SyncEngine<'a> {
    pub signer: &'a dyn ManifestSigner,
    pub store: Option<&'a ControllerStore>,
    pub audit: Option<&'a AuditPublisher>,
}

impl<'a> SyncEngine<'a> {
    pub async fn sync_repo(&self, config: &GitSyncConfig, dry_run: bool) -> Result<SyncReport, SyncError> {
        let git_commit = pull_latest_commit(config)?;
        let agents_root = config.repo_path.join(&config.agents_dir);
        let paths = discover_manifest_paths(&agents_root)?;
        let mut manifests = Vec::with_capacity(paths.len());
        for path in paths {
            let bytes = std::fs::read(&path).map_err(SyncError::Io)?;
            let manifest = parse_manifest(&bytes).map_err(|error| SyncError::Manifest {
                path: path.clone(),
                error,
            })?;
            validate_manifest_record(&manifest.record).map_err(|error| SyncError::Validation {
                path: path.clone(),
                error,
            })?;
            verify_manifest_signature(&manifest.record, &manifest.signature, &self.signer.verifying_key())
                .map_err(|error| SyncError::Manifest {
                    path: path.clone(),
                    error,
                })?;
            manifests.push((path, manifest));
        }

        validate_repo_monotonic_versions(&manifests)?;

        let mut applied = 0usize;
        for (path, manifest) in &manifests {
            if dry_run {
                info!(path = %path.display(), agent_id = %manifest.record.agent_id, "dry-run manifest ok");
                continue;
            }
            let Some(store) = self.store else {
                return Err(SyncError::Signer(
                    "KV store is required when dry_run=false".into(),
                ));
            };
            let prior = store.get_latest(&manifest.record.agent_id).await.map_err(SyncError::Kv)?;
            if let Some(existing) = &prior {
                monotonic_version(
                    &manifest.record.agent_id,
                    &existing.agent_version,
                    &manifest.record.agent_version,
                )
                .map_err(|error| SyncError::Validation {
                    path: path.clone(),
                    error,
                })?;
            }
            let now = OffsetDateTime::now_utc();
            let record = manifest.record.to_agent_record(now);
            let outcome = store.put_record(record.clone()).await.map_err(SyncError::Kv)?;
            applied += 1;

            if let Some(audit) = self.audit {
                let kind = classify_mutation(prior.as_ref(), &record);
                let digest = manifest_digest(manifest);
                let event = AuditEvent {
                    event_type: match kind {
                        crate::audit::RegistryMutationKind::Registered => "register",
                        crate::audit::RegistryMutationKind::VersionBump => "version_bump",
                        crate::audit::RegistryMutationKind::Deprecated => "deprecate",
                        crate::audit::RegistryMutationKind::Revoked => "revoke",
                    },
                    agent_id: &record.agent_id,
                    agent_version: &record.agent_version,
                    prior_lifecycle_state: prior.as_ref().map(|existing| lifecycle_label(existing.lifecycle_state)),
                    new_lifecycle_state: lifecycle_label(record.lifecycle_state),
                    manifest_digest: &digest,
                    operator: audit.operator(),
                    kv_revision: Some(outcome.latest_revision),
                    git_commit: &git_commit,
                };
                audit.emit(kind, event).await;
            }
        }

        Ok(SyncReport {
            git_commit,
            manifests_seen: manifests.len(),
            manifests_applied: if dry_run { 0 } else { applied },
            dry_run,
        })
    }
}

pub fn pull_latest_commit(config: &GitSyncConfig) -> Result<String, SyncError> {
    if config.git_remote.is_some() {
        run_git(&config.repo_path, &["fetch", "--all", "--prune"])?;
        run_git(
            &config.repo_path,
            &["checkout", config.git_ref.as_str()],
        )?;
        run_git(
            &config.repo_path,
            &["pull", "--ff-only", "origin", config.git_ref.as_str()],
        )?;
    }
    let output = run_git_output(&config.repo_path, &["rev-parse", "HEAD"])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn discover_manifest_paths(agents_root: &Path) -> Result<Vec<PathBuf>, SyncError> {
    if !agents_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_toml_paths(agents_root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_toml_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), SyncError> {
    for entry in std::fs::read_dir(dir).map_err(SyncError::Io)? {
        let entry = entry.map_err(SyncError::Io)?;
        let path = entry.path();
        if path.is_dir() {
            collect_toml_paths(&path, paths)?;
        } else if path.extension().is_some_and(|ext| ext == "toml") {
            paths.push(path);
        }
    }
    Ok(())
}

fn validate_repo_monotonic_versions(manifests: &[(PathBuf, SignedManifest)]) -> Result<(), SyncError> {
    let mut latest_by_agent: BTreeMap<&str, (&PathBuf, &SignedManifest)> = BTreeMap::new();
    for (path, manifest) in manifests {
        let agent_id = manifest.record.agent_id.as_str();
        if let Some((existing_path, existing)) = latest_by_agent.get(agent_id) {
            if !crate::manifest::version_gt(
                &manifest.record.agent_version,
                &existing.record.agent_version,
            ) {
                return Err(SyncError::Validation {
                    path: path.clone(),
                    error: ValidationError::MonotonicVersion {
                        agent_id: agent_id.to_string(),
                        current: existing.record.agent_version.clone(),
                        proposed: manifest.record.agent_version.clone(),
                    },
                });
            }
            if manifest.record.agent_version == existing.record.agent_version && **existing_path != *path {
                return Err(SyncError::Validation {
                    path: path.clone(),
                    error: ValidationError::Version(format!(
                        "duplicate agent_version `{}` for `{}`",
                        manifest.record.agent_version, agent_id
                    )),
                });
            }
        }
        latest_by_agent.insert(agent_id, (path, manifest));
    }
    Ok(())
}

fn manifest_digest(manifest: &SignedManifest) -> String {
    let body = toml::to_string(manifest).unwrap_or_default();
    let digest = Sha256::digest(body.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<(), SyncError> {
    let output = run_git_output(cwd, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(SyncError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn run_git_output(cwd: &Path, args: &[&str]) -> Result<Output, SyncError> {
    Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SyncError::Git("git executable not found on PATH".into())
            } else {
                SyncError::Io(error)
            }
        })
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use ed25519_dalek::SigningKey;
    use tempfile::TempDir;
    use trogon_agent_registry::LifecycleState;

    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;

    use crate::manifest::{ManifestRecord, SignedManifest, MANIFEST_VERSION, signing_payload};
    use crate::signer::FileManifestSigner;

    use super::*;

    fn init_git_repo(root: &Path) {
        for args in [
            &["init"][..],
            &["config", "user.email", "agent@example.com"],
            &["config", "user.name", "agent"],
        ] {
            assert!(Command::new("git").current_dir(root).args(args).status().unwrap().success());
        }
    }

    fn write_signed_manifest(
        root: &Path,
        relative: &str,
        record: ManifestRecord,
        signer: &FileManifestSigner,
    ) {
        let payload = signing_payload(&record).expect("payload");
        let signature = signer.sign_payload(&payload).expect("sign");
        let manifest = SignedManifest {
            manifest_version: MANIFEST_VERSION,
            record,
            signature,
        };
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, toml::to_string_pretty(&manifest).expect("toml")).expect("write");
    }

    #[test]
    fn dry_run_git_sync_validates_in_memory_repo() {
        let temp = TempDir::new().expect("tempdir");
        init_git_repo(temp.path());
        let signer = FileManifestSigner::from_pem(
            &SigningKey::from_bytes(&[9u8; 32])
                .to_pkcs8_pem(LineEnding::LF)
                .expect("pem"),
        )
        .expect("signer");

        write_signed_manifest(
            temp.path(),
            "agents/acme/oncall-agent.toml",
            ManifestRecord {
                agent_id: "acme/oncall-agent".into(),
                agent_version: "1.0.0".into(),
                agent_definition_digest: "sha256:abc123".into(),
                owner_team: "platform".into(),
                allowed_workloads: vec!["spiffe://acme.local/ns/prod/sa/oncall-agent".into()],
                allowed_tools: vec!["pagerduty.page".into()],
                allowed_audiences: vec!["mcp.server.pagerduty".into()],
                allowed_purposes: vec!["incident.response".into()],
                mesh_token_ttl_s: Some(120),
                metadata: serde_json::Value::Null,
                lifecycle_state: LifecycleState::Active,
            },
            &signer,
        );

        assert!(Command::new("git")
            .current_dir(temp.path())
            .args(["add", "."])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .current_dir(temp.path())
            .args(["commit", "-m", "seed"])
            .status()
            .unwrap()
            .success());

        let engine = SyncEngine {
            signer: &signer,
            store: None,
            audit: None,
        };
        let report = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(engine.sync_repo(
                &GitSyncConfig {
                    repo_path: temp.path().to_path_buf(),
                    agents_dir: PathBuf::from("agents"),
                    git_remote: None,
                    git_ref: "main".into(),
                },
                true,
            ))
            .expect("sync");

        assert_eq!(report.manifests_seen, 1);
        assert!(report.dry_run);
        assert_eq!(report.manifests_applied, 0);
    }
}
