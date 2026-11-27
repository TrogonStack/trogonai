use std::collections::BTreeMap;
use std::time::Duration;

use trogon_sandbox_contract::{FileContentHashing, FileOp, FileOpErrorKind, FileRecursion, FileSha256};
use trogon_sandbox_file_ops::{FileOpError, Manifest, ManifestEntry, ManifestSource};

use super::constants::{
    EXIT_NO_COMMAND, EXIT_NOT_FOUND, HASH_SCRIPT, MANIFEST_SCRIPT, MAX_MANIFEST_BYTES, SCAN_TIMEOUT,
};
use crate::envd::{Envd, EnvdTarget, ProcessRequest, ProcessResult};
use crate::watch::api_failure;

/// Reads manifests out of one sandbox through envd.
pub struct EnvdSource<'a> {
    pub envd: &'a Envd,
    pub target: &'a EnvdTarget,
    pub user: &'a str,
    pub timeout: Duration,
}

impl<'a> EnvdSource<'a> {
    pub fn new(envd: &'a Envd, target: &'a EnvdTarget, user: &'a str) -> Self {
        Self {
            envd,
            target,
            user,
            timeout: SCAN_TIMEOUT,
        }
    }

    async fn script(&self, script: &str, path: &str, recursion: FileRecursion) -> Result<ProcessResult, FileOpError> {
        let argv = vec![
            "bash".to_owned(),
            "-c".to_owned(),
            script.to_owned(),
            "bash".to_owned(),
            path.to_owned(),
            if recursion.is_recursive() { "1" } else { "0" }.to_owned(),
            (FileOp::TRACKING_MAX_ENTRIES + 1).to_string(),
            (MAX_MANIFEST_BYTES + 1).to_string(),
        ];
        let request = ProcessRequest {
            argv,
            env: BTreeMap::new(),
            cwd: None,
            user: self.user.to_owned(),
            timeout: self.timeout,
        };
        self.envd
            .run(self.target, &request)
            .await
            .map_err(|error| api_failure(&error, path))
    }

    async fn hashes(&self, path: &str, recursion: FileRecursion, manifest: &mut Manifest) -> Result<(), FileOpError> {
        let out = self.script(HASH_SCRIPT, path, recursion).await?;
        if out.exit_code == EXIT_NO_COMMAND {
            return Err(FileOpError::Unsupported {
                message: "the sandbox has no sha256sum, so a checkpoint cannot hash content".to_owned(),
            });
        }
        let out = checked(out, path)?;
        if out.stdout.len() > MAX_MANIFEST_BYTES {
            return Err(too_big(path));
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some((digest, name)) = line.split_once("  ")
                && let Ok(hash) = FileSha256::new(digest)
                && let Some(rebuilt) = manifest.get(name).map(|entry| with_hash(entry, hash))
            {
                manifest.insert(name, rebuilt);
            }
        }
        Ok(())
    }
}

/// `entry` with its content hash set; directories carry none.
fn with_hash(entry: &ManifestEntry, hash: FileSha256) -> ManifestEntry {
    if entry.is_directory() {
        return ManifestEntry::directory();
    }
    ManifestEntry::file(entry.size(), entry.stamp().map(str::to_owned), Some(hash))
}

/// The scan's output, or why it failed.
fn checked(out: ProcessResult, path: &str) -> Result<ProcessResult, FileOpError> {
    match out.exit_code {
        0 => Ok(out),
        EXIT_NOT_FOUND => Err(FileOpError::NotFound { path: path.to_owned() }),
        code => Err(FileOpError::failed(
            FileOpErrorKind::Io,
            format!(
                "scanning {path} exited {code}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        )),
    }
}

fn too_big(path: &str) -> FileOpError {
    FileOpError::too_large(format!(
        "{path} holds more than {} entries or {MAX_MANIFEST_BYTES} bytes of listing to track",
        FileOp::TRACKING_MAX_ENTRIES
    ))
}

impl ManifestSource for EnvdSource<'_> {
    async fn manifest(
        &self,
        path: &str,
        recursion: FileRecursion,
        hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpError> {
        if !path.starts_with('/') {
            return Err(FileOpError::invalid_path("a tracked path must be absolute"));
        }
        if path == "/" {
            return Err(FileOpError::invalid_path(
                "the sandbox root is too large to track; name a subdirectory",
            ));
        }
        let out = checked(self.script(MANIFEST_SCRIPT, path, recursion).await?, path)?;
        if out.stdout.len() > MAX_MANIFEST_BYTES {
            return Err(too_big(path));
        }
        let mut manifest = Manifest::from_stat_output(&String::from_utf8_lossy(&out.stdout));
        if manifest.len() > FileOp::TRACKING_MAX_ENTRIES {
            return Err(too_big(path));
        }
        if hashing.hashes_content() {
            self.hashes(path, recursion, &mut manifest).await?;
        }
        Ok(manifest)
    }
}
