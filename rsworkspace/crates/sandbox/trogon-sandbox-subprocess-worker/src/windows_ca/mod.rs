//! The persistent CA a Windows sandbox user trusts.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use trogon_sandbox_egress_proxy::tls_ca::{CaThumbprint, cert_thumbprint, generate_ca, validate_ca_pair};
use trogon_sandbox_egress_proxy::workload_trust_bundle;
use trogon_sandbox_windows_fence::{SandboxStatus, WindowsFence};
use trogon_std::env::{ReadEnv as _, SystemEnv};

use crate::lease_proxy::CaFilePaths;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCa {
    cert_pem: String,
    key_pem: String,
}

/// How long before its expiry a stored CA is replaced, and whether a valid one
/// is replaced anyway (`regenerate_within_days` and `force`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistentCaOptions {
    pub force: bool,
    pub regenerate_within_days: u32,
}

impl Default for PersistentCaOptions {
    fn default() -> Self {
        Self {
            force: false,
            regenerate_within_days: DEFAULT_REGENERATE_WITHIN_DAYS,
        }
    }
}

/// The CA of a state directory, ready for a session to sign with.
#[derive(Debug, Clone)]
pub struct PersistentCa {
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub thumbprint: CaThumbprint,
    pub generated: bool,
    pub trusted: bool,
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn atomic_write(path: &Path, data: &str) -> std::io::Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(name);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

struct Valid {
    stored: StoredCa,
    not_after: i64,
}

fn read_valid(json_path: &Path) -> Option<Valid> {
    let stored: StoredCa = serde_json::from_slice(&std::fs::read(json_path).ok()?).ok()?;
    let validated = validate_ca_pair(&stored.cert_pem, &stored.key_pem).ok()?;
    Some(Valid {
        not_after: validated.not_after.unix_timestamp(),
        stored,
    })
}

fn sweep_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.contains(".tmp.") && !name.starts_with(".trust.") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age.as_secs() > STALE_SECONDS);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `%ProgramData%\sandbox`, which the install
/// creates and whose access control keeps the sandbox user out of it.
pub fn windows_state_dir() -> anyhow::Result<PathBuf> {
    state_dir_under(SystemEnv.var_os("ProgramData"))
}

fn state_dir_under(program_data: Option<std::ffi::OsString>) -> anyhow::Result<PathBuf> {
    let program_data = program_data.context("ProgramData is not set")?;
    Ok(PathBuf::from(program_data).join("sandbox"))
}

/// The CA under `dir`, or under the state
/// directory's `ca` when `dir` is none, ensured and trusted by the sandbox
/// user. It never creates the state directory: that is the install's.
pub fn ensure_persistent_windows_ca(
    fence: &WindowsFence,
    status: &SandboxStatus,
    dir: Option<&Path>,
    options: PersistentCaOptions,
) -> anyhow::Result<PersistentCa> {
    anyhow::ensure!(
        status.user.provisioned && status.user.cred_present,
        "the sandbox user is not provisioned (user={}, cred={}), run windows-install first",
        status.user.provisioned,
        status.user.cred_present
    );
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => {
            let state_dir = windows_state_dir()?;
            anyhow::ensure!(
                state_dir.is_dir(),
                "the state directory {} does not exist, run windows-install first",
                state_dir.display()
            );
            state_dir.join("ca")
        }
    };
    ensure_persistent_ca(&dir, status.user.ca_cert_thumb.as_deref(), options, &mut |cert| {
        fence.trust_ca(cert).map_err(Into::into)
    })
}

/// The persistent CA of the machine's state directory, ensured and trusted by
/// the sandbox user.
pub fn persistent_ca_files(fence: &WindowsFence, status: &SandboxStatus) -> anyhow::Result<CaFilePaths> {
    let persistent = ensure_persistent_windows_ca(fence, status, None, PersistentCaOptions::default())?;
    Ok(CaFilePaths {
        cert: persistent.cert_path,
        key: persistent.key_path,
    })
}

/// The bundle a command trusts: the CA and the system roots, in a directory
/// of its own so the sandbox user can be granted just that.
#[derive(Debug)]
pub struct TrustBundle {
    dir: tempfile::TempDir,
    file: PathBuf,
}

impl TrustBundle {
    pub fn write(ca_cert: &Path, extra_ca_cert_paths: &[PathBuf]) -> anyhow::Result<Self> {
        let cert_pem = std::fs::read_to_string(ca_cert).with_context(|| format!("reading {}", ca_cert.display()))?;
        let dir = tempfile::Builder::new()
            .prefix("sandbox-ca-")
            .tempdir()
            .context("creating the trust bundle directory")?;
        let file = dir.path().join("ca.pem");
        std::fs::write(&file, workload_trust_bundle(&cert_pem, extra_ca_cert_paths))
            .with_context(|| format!("writing {}", file.display()))?;
        Ok(Self { dir, file })
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

/// Reuse the CA in `dir` when it is valid
/// and not expiring within the renewal window of `options` (unless forced), otherwise
/// generate one; then trust it through `trust` when it was generated or `trusted_thumbprint` (the one the
/// sandbox user's status records) is another. `trust` is given a path of its
/// own that is removed afterwards.
pub fn ensure_persistent_ca(
    dir: &Path,
    trusted_thumbprint: Option<&str>,
    options: PersistentCaOptions,
    trust: &mut dyn FnMut(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<PersistentCa> {
    let json_path = dir.join("ca.json");
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");

    let window = i64::from(options.regenerate_within_days) * SECONDS_PER_DAY;
    let reusable = read_valid(&json_path).filter(|on| !options.force && on.not_after - now_seconds() > window);
    let (valid, generated) = match reusable {
        Some(on) => (on, false),
        None => {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            sweep_stale(dir);
            let generated = generate_ca(Some(CA_COMMON_NAME), None)
                .map_err(|error| anyhow::anyhow!("generating the persistent CA: {error}"))?;
            let json = serde_json::to_string(&StoredCa {
                cert_pem: generated.cert_pem.clone(),
                key_pem: generated.key_pem.clone(),
            })?;
            atomic_write(&json_path, &json).with_context(|| format!("writing {}", json_path.display()))?;
            let on = read_valid(&json_path).unwrap_or(Valid {
                not_after: i64::MAX,
                stored: StoredCa {
                    cert_pem: generated.cert_pem,
                    key_pem: generated.key_pem,
                },
            });
            (on, true)
        }
    };

    atomic_write(&cert_path, &valid.stored.cert_pem)?;
    atomic_write(&key_path, &valid.stored.key_pem)?;
    let thumbprint = cert_thumbprint(&valid.stored.cert_pem)
        .map_err(|error| anyhow::anyhow!("reading the CA thumbprint: {error}"))?;
    let trusted = generated || trusted_thumbprint != Some(thumbprint.as_str());
    if trusted {
        let temporary = dir.join(format!(
            ".trust.{}.{}.pem",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis())
        ));
        std::fs::write(&temporary, &valid.stored.cert_pem)?;
        let outcome = trust(&temporary);
        let _ = std::fs::remove_file(&temporary);
        outcome?;
    }
    Ok(PersistentCa {
        cert_pem: valid.stored.cert_pem,
        key_pem: valid.stored.key_pem,
        cert_path,
        key_path,
        thumbprint,
        generated,
        trusted,
    })
}

mod constants;
#[cfg(test)]
mod tests;
use constants::{CA_COMMON_NAME, DEFAULT_REGENERATE_WITHIN_DAYS, SECONDS_PER_DAY, STALE_SECONDS};
