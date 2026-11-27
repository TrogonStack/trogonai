//! Structured file operations inside a running container.
//!
//! The container filesystem is not visible on the host, so the operations that walk a tree
//! (list, search, archive download, and the read half of an edit) stage the target out of
//! the container with the Docker archive API into a scratch directory and run it through
//! [`trogon_sandbox_file_ops::FsRoot`]. Everything else is a small `sh` script run with `exec`:
//! ranged reads, writes, stat, remove, mkdir, rename and archive upload.
//!
//! Limits, all reported as typed replies instead of silent truncation:
//! - `as_user` is unsupported: the worker has no per-operation identity switch for files.
//! - Writes are append-only: `offset` must equal the file's current size (or 0 with
//!   `truncate`), which is exactly what a chunked upload does.
//! - A staged tree may hold at most [`STAGING_CAP_BYTES`] of archive stream; a bigger tree
//!   answers `TooLarge` and is moved file by file with ranged reads and writes.
//! - Absolute container paths only, and symlinks inside a staged tree that point outside it
//!   are not followed.
//! - Change tracking and watching never stage a tree. They fingerprint it with one `find`
//!   and `stat` script per scan ([`ContainerSource`]), so they work on a tree too big to
//!   stage, but a scan costs one `exec` per poll interval while a watch poll is parked.
//!   When the image has `inotifywait`, each watch keeps one long-lived `inotifywait -m`
//!   exec ([`feed`]) whose events wake a parked poll at once and recover the path created
//!   and deleted between scans and the rewrite that keeps size and timestamp; the feed's
//!   own residuals are listed there. A watch whose feed is not running blocks on a one-shot
//!   `inotifywait` between scans, and an image without it is remembered per lease
//!   ([`InotifyMissing`]) and falls back to sleeping one scan interval, where events are
//!   net differences between scans and a rewrite within one filesystem timestamp tick that
//!   keeps the size is invisible unless the checkpoint asked for content hashes.
//! - Without a checkpoint, `changes` compares `/workspace` with the manifest recorded when
//!   the lease was created ([`record_start_baseline`]), so it is exact there and ignores what
//!   the worker wrote while provisioning. Paths outside `/workspace` come from the container's
//!   own diff against its image, so they include what the worker wrote there. When no
//!   baseline could be recorded (no `/workspace`, or more entries than can be tracked),
//!   `changes` falls back to the container's own diff against its image (Docker `changes`),
//!   which also lists whatever the worker wrote while provisioning.
//! - An edit reads a staged copy and writes the result back with a temp file and rename, so
//!   it is not serialised against a process inside the container writing the same file.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bollard::Docker;
use futures::StreamExt;
use trogon_sandbox_contract::{
    Base64Data, CommandOutput, FileContentHashing, FileKind, FileOp, FileOpErrorKind, FileOpResponse, FileOverwrite,
    FileParentCreation, FilePresence, FileReadEnd, FileRecursion, FileResultTruncation, FileSha256, FileStat,
    FileWriteTruncation,
};
use trogon_sandbox_file_ops::{
    ArchiveSummary, ChangeHint, EditOutcome, EditSpec, FileOpError, FsRoot, Manifest, ManifestDiff, ManifestEntry,
    ManifestSource, StreamHasher,
};

mod constants;
mod refusal;
pub(crate) use refusal::Refusal;

use constants::{
    CHECKSUM_PAGE_BYTES, EXIT_EXISTS, EXIT_IS_DIRECTORY, EXIT_NO_COMMAND, EXIT_NO_TAR, EXIT_NOT_FOUND,
    EXIT_OFFSET_MISMATCH, HASH_SCRIPT, INOTIFY_MAX_WAIT_SECONDS, INOTIFY_MISSING_EXIT, INOTIFY_WAIT_SCRIPT,
    MANIFEST_SCRIPT,
};
pub(crate) use constants::{STAGING_CAP_BYTES, WORKSPACE_PATH};

pub(crate) enum Download {
    Tar(Vec<u8>),
    Missing,
    TooLarge,
    Failed(String),
}

/// What the file operations need from a container. The Docker implementation is
/// [`DockerContainer`]; tests substitute an in-memory one.
pub(crate) trait ContainerAccess {
    /// Streams the archive of `path`, giving up once it passes `cap` bytes.
    async fn download(&self, path: &str, cap: u64) -> Download;

    /// Runs `sh -c script sh args...` with optional stdin.
    async fn sh(&self, script: &str, args: &[&str], stdin: Option<Vec<u8>>) -> anyhow::Result<CommandOutput>;

    /// What the container's filesystem differs by from its image, as `(path, kind)` with
    /// kind 0 modified, 1 added, 2 deleted. `None` when the backend cannot say.
    async fn changes_since_creation(&self) -> Option<Result<Vec<(String, u8)>, String>> {
        None
    }
}

pub(crate) struct DockerContainer<'a> {
    pub docker: &'a Docker,
    pub container_id: String,
}

impl ContainerAccess for DockerContainer<'_> {
    async fn download(&self, path: &str, cap: u64) -> Download {
        let options = bollard::query_parameters::DownloadFromContainerOptionsBuilder::new()
            .path(path)
            .build();
        let mut stream = self.docker.download_from_container(&self.container_id, Some(options));
        let mut tar = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if tar.len() as u64 + bytes.len() as u64 > cap {
                        return Download::TooLarge;
                    }
                    tar.extend_from_slice(&bytes);
                }
                Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                    return Download::Missing;
                }
                Err(e) => return Download::Failed(e.to_string()),
            }
        }
        Download::Tar(tar)
    }

    async fn changes_since_creation(&self) -> Option<Result<Vec<(String, u8)>, String>> {
        Some(
            self.docker
                .container_changes(&self.container_id)
                .await
                .map(|changes| {
                    changes
                        .unwrap_or_default()
                        .into_iter()
                        .map(|c| (c.path, c.kind as i32 as u8))
                        .collect()
                })
                .map_err(|e| e.to_string()),
        )
    }

    async fn sh(&self, script: &str, args: &[&str], stdin: Option<Vec<u8>>) -> anyhow::Result<CommandOutput> {
        let mut command = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
        command.push("sh".to_string());
        command.extend(args.iter().map(|a| (*a).to_string()));
        crate::adapter::DockerWorkerProvider::exec_for_file_op(self.docker, &self.container_id, command, stdin).await
    }
}

/// The reply a failed manifest scan gives back to the tracking state machine.
fn response_error(response: FileOpResponse) -> FileOpError {
    match response {
        FileOpResponse::NotFound { path } => FileOpError::NotFound { path },
        FileOpResponse::Failed { kind, message } => FileOpError::failed(kind, message),
        FileOpResponse::Unsupported { message } => FileOpError::Unsupported { message },
        FileOpResponse::Denied(error) => FileOpError::failed(FileOpErrorKind::Io, error.to_string()),
        _ => FileOpError::failed(FileOpErrorKind::Io, "unexpected reply"),
    }
}

fn decode(data: &Base64Data) -> Result<Vec<u8>, Refusal> {
    STANDARD.decode(data.as_str()).map_err(|e| {
        failed(
            FileOpErrorKind::InvalidRequest,
            format!("data is not valid base64: {e}"),
        )
        .into()
    })
}

fn encode(bytes: &[u8]) -> Result<Base64Data, Refusal> {
    Base64Data::new(STANDARD.encode(bytes))
        .map_err(|e| failed(FileOpErrorKind::Io, format!("encoding reply data: {e}")).into())
}

fn failed(kind: FileOpErrorKind, message: impl Into<String>) -> FileOpResponse {
    FileOpResponse::Failed {
        kind,
        message: message.into(),
    }
}

/// Normalises an absolute container path lexically. `..` cannot climb above `/`.
/// Normalises an absolute container path, for callers outside this module.
pub(crate) fn absolute_path(path: &str) -> Result<String, Refusal> {
    absolute(path)
}

fn absolute(path: &str) -> Result<String, Refusal> {
    if path.contains('\0') {
        return Err(failed(FileOpErrorKind::InvalidPath, "path contains a NUL").into());
    }
    if !path.starts_with('/') {
        return Err(failed(
            FileOpErrorKind::InvalidPath,
            format!("{path} is not absolute; the docker provider needs absolute container paths"),
        )
        .into());
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(failed(
                        FileOpErrorKind::InvalidPath,
                        format!("path {path} climbs above the container root"),
                    )
                    .into());
                }
            }
            other => parts.push(other),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

/// Maps a failed script to the typed reply a caller can act on.
fn script_failure(out: &CommandOutput, path: &str) -> FileOpResponse {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lower = stderr.to_lowercase();
    match out.exit_code {
        EXIT_NOT_FOUND => return FileOpResponse::NotFound { path: path.into() },
        EXIT_IS_DIRECTORY => {
            return failed(FileOpErrorKind::IsADirectory, format!("{path} is a directory"));
        }
        EXIT_EXISTS => {
            return failed(FileOpErrorKind::AlreadyExists, format!("{path} already exists"));
        }
        _ => {}
    }
    let kind = if lower.contains("permission denied") || lower.contains("read-only") {
        FileOpErrorKind::PermissionDenied
    } else if lower.contains("not a directory") {
        FileOpErrorKind::NotADirectory
    } else if lower.contains("is a directory") {
        FileOpErrorKind::IsADirectory
    } else if lower.contains("not empty") {
        FileOpErrorKind::DirectoryNotEmpty
    } else if lower.contains("no such file") {
        return FileOpResponse::NotFound { path: path.into() };
    } else {
        FileOpErrorKind::Io
    };
    failed(
        kind,
        format!(
            "{path}: exit {} ({})",
            out.exit_code,
            stderr.trim().chars().take(300).collect::<String>()
        ),
    )
}

fn denied(e: anyhow::Error) -> FileOpResponse {
    FileOpResponse::Denied(trogon_sandbox_contract::LeaseError::Backend { message: e.to_string() })
}

/// Runs one operation against a container.
pub(crate) async fn execute<C: ContainerAccess>(container: &C, op: &FileOp, as_user: bool) -> FileOpResponse {
    if as_user {
        return FileOpResponse::Unsupported {
            message: "the docker provider cannot run a file operation as another user".into(),
        };
    }
    match run(container, op).await {
        Ok(response) | Err(response) => response,
    }
}

type Reply = Result<FileOpResponse, FileOpResponse>;

async fn run<C: ContainerAccess>(container: &C, op: &FileOp) -> Reply {
    match op {
        FileOp::Stat { path } => stat(container, path).await,
        FileOp::Read {
            path,
            offset,
            length,
            max_bytes,
        } => read(container, path, *offset, *length, *max_bytes).await,
        FileOp::Write {
            path,
            data,
            offset,
            truncate,
            mode,
            create_parents,
        } => {
            let bytes = decode(data)?;
            write(container, path, &bytes, *offset, *truncate, *mode, *create_parents).await
        }
        FileOp::Edit {
            path,
            find,
            replace,
            expected_occurrences,
        } => edit(container, path, find, replace, *expected_occurrences).await,
        FileOp::Remove { path, recursive } => remove(container, path, *recursive).await,
        FileOp::Mkdir { path, parents } => mkdir(container, path, *parents).await,
        FileOp::Rename { from, to, overwrite } => rename(container, from, to, *overwrite).await,
        FileOp::ArchivePut { path, data } => {
            let bytes = decode(data)?;
            archive_put(container, path, &bytes).await
        }
        FileOp::Checksum { path, offset, length } => checksum(container, path, *offset, *length).await,
        // Tracking keeps state across calls, so the provider serves it through `Tracking`
        // with a `ContainerSource`; reaching here means a caller skipped that.
        FileOp::Checkpoint { .. }
        | FileOp::Changes { .. }
        | FileOp::WatchStart { .. }
        | FileOp::WatchPoll { .. }
        | FileOp::WatchStop { .. } => Err(FileOpResponse::Unsupported {
            message: "this file operation needs worker-side tracking state".into(),
        }),
        FileOp::List { path, .. } | FileOp::Search { path, .. } | FileOp::ArchiveGet { path, .. } => {
            let path = absolute(path)?;
            let staged = stage(container, &path).await?;
            let mut op = op.clone();
            match &mut op {
                FileOp::List { path: p, .. } | FileOp::Search { path: p, .. } | FileOp::ArchiveGet { path: p, .. } => {
                    *p = path
                }
                _ => {}
            }
            Ok(run_staged(staged, op).await)
        }
    }
}

/// A container's filesystem seen as a manifest source: one `find` and `stat` per scan.
pub(crate) struct ContainerSource<'a, C: ContainerAccess> {
    pub container: &'a C,
    /// Where to remember that this lease's image has no `inotifywait`, keyed by lease id.
    /// `None` probes on every wait.
    pub inotify_missing: Option<(&'a InotifyMissing, &'a str)>,
    /// Live inotify feeds by watch id. `None`, or a watch without a live feed, waits on
    /// [`ManifestSource::wait_for_change`].
    pub feeds: Option<&'a feed::WatchFeeds>,
}

/// Leases whose image has no `inotifywait`, so a parked watch skips the probe `exec`
/// and sleeps one scan interval straight away.
#[derive(Default)]
pub(crate) struct InotifyMissing(std::sync::Mutex<std::collections::HashSet<String>>);

impl InotifyMissing {
    pub(crate) fn contains(&self, lease_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(lease_id)
    }

    fn insert(&self, lease_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(lease_id.to_string());
    }

    /// Drops what is known about `lease_id`, once the lease ends.
    pub(crate) fn forget(&self, lease_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(lease_id);
    }
}

/// Records the workspace as it stands now as the lease-start baseline. Best effort: a
/// container without the directory, or with too many entries to track, keeps no baseline and
/// `changes` falls back to the Docker diff. A failed scan never fails the lease.
pub(crate) async fn record_start_baseline<S: ManifestSource>(
    tracking: &trogon_sandbox_file_ops::Tracking,
    lease: &str,
    source: &S,
) {
    match source
        .manifest(WORKSPACE_PATH, FileRecursion::Recursive, FileContentHashing::Metadata)
        .await
    {
        Ok(manifest) => tracking.set_start_baseline(lease, WORKSPACE_PATH, manifest),
        Err(_) => tracking.forget(lease),
    }
}

impl<C: ContainerAccess> ContainerSource<'_, C> {
    async fn scan(
        &self,
        path: &str,
        recursion: FileRecursion,
        hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpResponse> {
        let path = absolute(path)?;
        if path == "/" {
            return Err(failed(
                FileOpErrorKind::InvalidPath,
                "the container root is too large to track; name a subdirectory",
            ));
        }
        let ceiling = (FileOp::TRACKING_MAX_ENTRIES + 1).to_string();
        let flag = if recursion.is_recursive() { "1" } else { "0" };
        let out = exec(self.container, MANIFEST_SCRIPT, &[&path, flag, &ceiling], None).await?;
        if out.exit_code != 0 {
            return Err(script_failure(&out, &path));
        }
        let too_big = || {
            failed(
                FileOpErrorKind::TooLarge,
                format!(
                    "{path} holds more than {} entries to track",
                    FileOp::TRACKING_MAX_ENTRIES
                ),
            )
        };
        if out.truncated.is_truncated() {
            return Err(too_big());
        }
        let manifest = Manifest::from_stat_output(&String::from_utf8_lossy(&out.stdout));
        if manifest.len() > FileOp::TRACKING_MAX_ENTRIES {
            return Err(too_big());
        }
        if hashing.hashes_content() {
            let out = exec(self.container, HASH_SCRIPT, &[&path, flag, &ceiling], None).await?;
            if out.exit_code == EXIT_NO_COMMAND {
                return Err(FileOpResponse::Unsupported {
                    message: "the sandbox image has no sha256sum, so a checkpoint cannot hash content".into(),
                });
            }
            if out.exit_code != 0 {
                return Err(script_failure(&out, &path));
            }
            let mut digests = std::collections::HashMap::new();
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some((digest, name)) = line.split_once("  ")
                    && let Ok(digest) = FileSha256::new(digest)
                {
                    digests.insert(name.to_string(), digest);
                }
            }
            return Ok(manifest
                .iter()
                .map(|(name, entry)| {
                    let hashed = match digests.remove(name) {
                        Some(digest) if !entry.is_directory() => {
                            ManifestEntry::file(entry.size(), entry.stamp().map(str::to_string), Some(digest))
                        }
                        _ => entry.clone(),
                    };
                    (name.clone(), hashed)
                })
                .collect());
        }
        Ok(manifest)
    }
}

impl<C: ContainerAccess> ManifestSource for ContainerSource<'_, C> {
    async fn manifest(
        &self,
        path: &str,
        recursion: FileRecursion,
        hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpError> {
        self.scan(path, recursion, hashing).await.map_err(response_error)
    }

    async fn changes_since_start(&self) -> Option<Result<ManifestDiff, FileOpError>> {
        let changes = match self.container.changes_since_creation().await? {
            Ok(changes) => changes,
            Err(message) => return Some(Err(FileOpError::failed(FileOpErrorKind::Io, message))),
        };
        let mut diff = ManifestDiff::default();
        for (path, kind) in changes {
            match kind {
                1 => diff.added.push(path),
                2 => diff.deleted.push(path),
                _ => diff.modified.push(path),
            }
        }
        diff.added.sort();
        diff.modified.sort();
        diff.deleted.sort();
        Some(Ok(diff))
    }

    fn scan_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(1000)
    }

    /// Waits on the watch's live inotify feed when it has one, so a parked poll wakes on
    /// the change itself; otherwise falls back to [`Self::wait_for_change`].
    async fn wait_for_watch(&self, watch_id: &str, path: &str, recursion: FileRecursion, timeout: std::time::Duration) {
        match self.feeds.and_then(|feeds| feeds.get(watch_id)) {
            Some(feed) if feed.is_live() => feed.wait(timeout).await,
            _ => self.wait_for_change(path, recursion, timeout).await,
        }
    }

    fn drain_hints(&self, watch_id: &str) -> Vec<ChangeHint> {
        self.feeds
            .and_then(|feeds| feeds.get(watch_id))
            .map(|feed| feed.drain())
            .unwrap_or_default()
    }

    /// Blocks on `inotifywait` inside the container when the image has it, so a parked
    /// poll rescans as soon as the tree changes instead of on the next 1s tick. Without
    /// it, or for a wait under a second (its timeout counts whole seconds and 0 means
    /// forever), this sleeps one scan interval like any other source. A lease found
    /// without it is remembered, so later waits skip the probe.
    async fn wait_for_change(&self, path: &str, recursion: FileRecursion, timeout: std::time::Duration) {
        let fallback = timeout.min(self.scan_interval());
        let seconds = timeout.as_secs().min(INOTIFY_MAX_WAIT_SECONDS);
        let known_missing = self
            .inotify_missing
            .is_some_and(|(missing, lease_id)| missing.contains(lease_id));
        let path = match absolute(path) {
            Ok(path) if seconds > 0 && !known_missing => path,
            _ => {
                tokio::time::sleep(fallback).await;
                return;
            }
        };
        let flag = if recursion.is_recursive() { "1" } else { "0" };
        let seconds = seconds.to_string();
        let woke = match exec(self.container, INOTIFY_WAIT_SCRIPT, &[&path, flag, &seconds], None).await {
            // 0 is a change, 2 the timeout passing: either way the wait is over.
            Ok(out) if matches!(out.exit_code, 0 | 2) => true,
            Ok(out) => {
                if out.exit_code == INOTIFY_MISSING_EXIT
                    && let Some((missing, lease_id)) = self.inotify_missing
                {
                    missing.insert(lease_id);
                }
                false
            }
            Err(_) => false,
        };
        if !woke {
            tokio::time::sleep(fallback).await;
        }
    }
}

struct Staged {
    dir: tempfile::TempDir,
}

/// Downloads `path` out of the container into a scratch directory laid out like the
/// container filesystem, so [`FsRoot`] can serve it under the same absolute path.
async fn stage<C: ContainerAccess>(container: &C, path: &str) -> Result<Staged, FileOpResponse> {
    if path == "/" {
        return Err(failed(
            FileOpErrorKind::InvalidPath,
            "the container root is too large to stage; name a subdirectory",
        ));
    }
    let tar = match container.download(path, STAGING_CAP_BYTES).await {
        Download::Tar(tar) => tar,
        Download::Missing => return Err(FileOpResponse::NotFound { path: path.into() }),
        Download::TooLarge => {
            return Err(failed(
                FileOpErrorKind::TooLarge,
                format!("{path} holds more than {STAGING_CAP_BYTES} bytes; use ranged reads and writes on its files"),
            ));
        }
        Download::Failed(message) => {
            return Err(failed(FileOpErrorKind::Io, format!("{path}: {message}")));
        }
    };
    let dir = tempfile::TempDir::new().map_err(|e| failed(FileOpErrorKind::Io, format!("staging directory: {e}")))?;
    let parent = Path::new(path).parent().unwrap_or(Path::new("/")).to_path_buf();
    let target = dir.path().join(parent.strip_prefix("/").unwrap_or(&parent));
    let unpacked = tokio::task::spawn_blocking(move || unpack(&tar, &target))
        .await
        .map_err(|e| failed(FileOpErrorKind::Io, format!("staging task failed: {e}")))?;
    unpacked.map_err(|e| failed(FileOpErrorKind::Io, format!("staging {path}: {e}")))?;
    Ok(Staged { dir })
}

/// Unpacks files, directories and symlinks. Device nodes, fifos and hard links are skipped,
/// and `unpack_in` refuses entries that would land outside `target`.
fn unpack(tar: &[u8], target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    let mut archive = tar::Archive::new(std::io::Cursor::new(tar));
    archive.set_preserve_permissions(true);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_file() || kind.is_dir() || kind.is_symlink() {
            entry.unpack_in(target)?;
        }
    }
    Ok(())
}

async fn run_staged(staged: Staged, op: FileOp) -> FileOpResponse {
    let root = staged.dir.path().to_path_buf();
    let outcome = tokio::task::spawn_blocking(move || match FsRoot::new(&root, None) {
        Ok(fs) => fs.execute(&op),
        Err(e) => failed(FileOpErrorKind::Io, format!("staging root: {e}")),
    })
    .await;
    outcome.unwrap_or_else(|e| failed(FileOpErrorKind::Io, format!("file operation task failed: {e}")))
}

async fn exec<C: ContainerAccess>(
    container: &C,
    script: &str,
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> Result<CommandOutput, FileOpResponse> {
    container.sh(script, args, stdin).await.map_err(denied)
}

async fn stat<C: ContainerAccess>(container: &C, path: &str) -> Reply {
    let path = absolute(path)?;
    let out = exec(
        container,
        r#"[ -e "$1" ] || [ -L "$1" ] || exit 44; stat -c '%F|%s|%a|%Y' "$1""#,
        &[&path],
        None,
    )
    .await?;
    if out.exit_code != 0 {
        return Err(script_failure(&out, &path));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.trim().split('|');
    let kind_text = fields.next().unwrap_or_default();
    let kind = if kind_text.contains("directory") {
        FileKind::Directory
    } else if kind_text.contains("symbolic link") {
        FileKind::Symlink
    } else if kind_text.contains("regular") {
        FileKind::File
    } else {
        FileKind::Other
    };
    let size = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mode = fields.next().and_then(|s| u32::from_str_radix(s, 8).ok());
    let modified = fields
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .map(|s| s.saturating_mul(1000));
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string());
    Ok(FileOpResponse::Stat {
        stat: FileStat {
            path,
            name,
            kind,
            size,
            mode,
            modified_unix_ms: modified,
            symlink_target: None,
        },
    })
}

/// Hashes a byte range by paging it through ranged reads, so the digest needs nothing from
/// the image beyond what reads already use.
async fn checksum<C: ContainerAccess>(container: &C, path: &str, offset: u64, length: Option<u64>) -> Reply {
    let mut hasher = StreamHasher::new();
    let mut hashed = 0u64;
    let mut total_size: u64;
    loop {
        let wanted = length.map_or(CHECKSUM_PAGE_BYTES, |l| {
            l.saturating_sub(hashed).min(CHECKSUM_PAGE_BYTES)
        });
        let page = read(
            container,
            path,
            offset.saturating_add(hashed),
            Some(wanted),
            Some(CHECKSUM_PAGE_BYTES),
        )
        .await?;
        let FileOpResponse::Content {
            data,
            total_size: total,
            eof,
            ..
        } = page
        else {
            return Err(failed(FileOpErrorKind::Io, "unexpected read reply"));
        };
        total_size = total;
        let bytes = decode(&data)?;
        hasher.update(&bytes);
        hashed += bytes.len() as u64;
        if eof.reached_end() || bytes.is_empty() || length.is_some_and(|l| hashed >= l) {
            break;
        }
    }
    Ok(FileOpResponse::Checksum {
        sha256: hasher
            .finish()
            .map_err(|e| failed(FileOpErrorKind::Io, format!("hashing: {e}")))?,
        bytes: hashed,
        total_size,
    })
}

async fn read<C: ContainerAccess>(
    container: &C,
    path: &str,
    offset: u64,
    length: Option<u64>,
    max_bytes: Option<u64>,
) -> Reply {
    let path = absolute(path)?;
    let size = exec(
        container,
        r#"[ -e "$1" ] || exit 44; [ -d "$1" ] && exit 21; wc -c < "$1""#,
        &[&path],
        None,
    )
    .await?;
    if size.exit_code != 0 {
        return Err(script_failure(&size, &path));
    }
    let total: u64 = String::from_utf8_lossy(&size.stdout)
        .trim()
        .parse()
        .map_err(|_| failed(FileOpErrorKind::Io, format!("{path}: unreadable size")))?;
    let available = total.saturating_sub(offset);
    let wanted = length.map_or(available, |l| l.min(available));
    let cap = max_bytes.unwrap_or(FileOp::MAX_READ_BYTES).min(FileOp::MAX_READ_BYTES);
    let count = wanted.min(cap);
    let mut data = Vec::new();
    if count > 0 {
        let from = (offset + 1).to_string();
        let count_text = count.to_string();
        let out = exec(
            container,
            r#"tail -c +"$2" "$1" | head -c "$3""#,
            &[&path, &from, &count_text],
            None,
        )
        .await?;
        if out.exit_code != 0 {
            return Err(script_failure(&out, &path));
        }
        data = out.stdout;
    }
    let end = offset.saturating_add(data.len() as u64);
    Ok(FileOpResponse::Content {
        offset,
        total_size: total,
        eof: FileReadEnd::from(end >= total),
        truncated: FileResultTruncation::from(count < wanted),
        data: encode(&data)?,
    })
}

async fn write<C: ContainerAccess>(
    container: &C,
    path: &str,
    data: &[u8],
    offset: u64,
    truncate: FileWriteTruncation,
    mode: Option<u32>,
    create_parents: FileParentCreation,
) -> Reply {
    if data.len() as u64 > FileOp::MAX_WRITE_BYTES {
        return Err(failed(
            FileOpErrorKind::TooLarge,
            format!(
                "write of {} bytes exceeds the {} byte per-request limit; send the file in chunks",
                data.len(),
                FileOp::MAX_WRITE_BYTES
            ),
        ));
    }
    let path = absolute(path)?;
    if path == "/" {
        return Err(failed(FileOpErrorKind::IsADirectory, "cannot write the root"));
    }
    let script = r#"
[ "$5" = 1 ] && { mkdir -p "$(dirname "$1")" || exit $?; }
[ -d "$1" ] && exit 21
[ "$3" = 1 ] && { : > "$1" || exit $?; }
if [ -e "$1" ]; then cur=$(wc -c < "$1"); else cur=0; fi
[ "$cur" = "$2" ] || exit 22
cat >> "$1" || exit $?
[ -n "$4" ] && { chmod "$4" "$1" || exit $?; }
wc -c < "$1"
"#;
    let offset_text = offset.to_string();
    let mode_text = mode.map(|m| format!("{:o}", m & 0o7777)).unwrap_or_default();
    let out = exec(
        container,
        script,
        &[
            &path,
            &offset_text,
            if truncate.truncates() { "1" } else { "0" },
            &mode_text,
            if create_parents.creates() { "1" } else { "0" },
        ],
        Some(data.to_vec()),
    )
    .await?;
    if out.exit_code == EXIT_OFFSET_MISMATCH {
        return Err(failed(
            FileOpErrorKind::InvalidRequest,
            format!(
                "the docker provider writes append-only: offset {offset} is not the current size of {path} (use offset 0 with truncate to start a file)"
            ),
        ));
    }
    if out.exit_code != 0 {
        return Err(script_failure(&out, &path));
    }
    let size = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(offset + data.len() as u64);
    Ok(FileOpResponse::Written {
        bytes: data.len() as u64,
        size,
    })
}

async fn edit<C: ContainerAccess>(container: &C, path: &str, find: &str, replace: &str, expected: u32) -> Reply {
    let path = absolute(path)?;
    let staged = stage(container, &path).await?;
    let relative = path.trim_start_matches('/').to_string();
    let staged_file = staged.dir.path().join(&relative);
    let metadata = tokio::fs::symlink_metadata(&staged_file)
        .await
        .map_err(|_| FileOpResponse::NotFound { path: path.clone() })?;
    if metadata.is_dir() {
        return Err(failed(FileOpErrorKind::IsADirectory, format!("{path} is a directory")));
    }
    let spec = EditSpec::new(find, replace, expected)
        .map_err(|e| failed(FileOpErrorKind::InvalidRequest, format!("edit: {e}")))?;
    let outcome = {
        let file = staged_file.clone();
        tokio::task::spawn_blocking(move || -> Result<_, Refusal> {
            let content =
                std::fs::read(&file).map_err(|e| failed(FileOpErrorKind::Io, format!("reading staged copy: {e}")))?;
            if content.len() as u64 > FileOp::MAX_READ_BYTES {
                return Err(failed(
                    FileOpErrorKind::TooLarge,
                    format!("file is larger than the {} byte edit limit", FileOp::MAX_READ_BYTES),
                )
                .into());
            }
            spec.apply(&content).map_err(|e| Refusal::from(FileOpResponse::from(e)))
        })
        .await
        .map_err(|e| failed(FileOpErrorKind::Io, format!("edit task failed: {e}")))??
    };
    let (content, occurrences) = match outcome {
        EditOutcome::Rejected { occurrences } => {
            return Ok(FileOpResponse::EditRejected { occurrences, expected });
        }
        EditOutcome::Replaced { content, occurrences } => (content, occurrences),
    };
    let size = content.len() as u64;
    let script = r#"
tmp="$1.edit.$$"
cat > "$tmp" || { rm -f "$tmp"; exit 1; }
m=$(stat -c %a "$1" 2>/dev/null) && chmod "$m" "$tmp"
mv -f "$tmp" "$1" || { rm -f "$tmp"; exit 1; }
"#;
    let out = exec(container, script, &[&path], Some(content)).await?;
    if out.exit_code != 0 {
        return Err(script_failure(&out, &path));
    }
    Ok(FileOpResponse::Edited { occurrences, size })
}

async fn remove<C: ContainerAccess>(container: &C, path: &str, recursion: FileRecursion) -> Reply {
    let path = absolute(path)?;
    if path == "/" {
        return Err(failed(FileOpErrorKind::InvalidPath, "cannot remove the container root"));
    }
    let script = r#"
if [ ! -e "$1" ] && [ ! -L "$1" ]; then echo missing; exit 0; fi
if [ -d "$1" ] && [ ! -L "$1" ]; then
  if [ "$2" = 1 ]; then rm -rf "$1"; else rmdir "$1"; fi
else
  rm -f "$1"
fi
"#;
    let out = exec(
        container,
        script,
        &[&path, if recursion.is_recursive() { "1" } else { "0" }],
        None,
    )
    .await?;
    if out.exit_code != 0 {
        return Err(script_failure(&out, &path));
    }
    let existed = FilePresence::from(!String::from_utf8_lossy(&out.stdout).contains("missing"));
    Ok(FileOpResponse::Removed { existed })
}

async fn mkdir<C: ContainerAccess>(container: &C, path: &str, parents: FileParentCreation) -> Reply {
    let path = absolute(path)?;
    let script = r#"
if [ -d "$1" ]; then echo exists; exit 0; fi
[ -e "$1" ] && exit 17
if [ "$2" = 1 ]; then mkdir -p "$1"; else mkdir "$1"; fi
"#;
    let out = exec(
        container,
        script,
        &[&path, if parents.creates() { "1" } else { "0" }],
        None,
    )
    .await?;
    if out.exit_code != 0 {
        return Err(script_failure(&out, &path));
    }
    Ok(FileOpResponse::Created {
        already_existed: FilePresence::from(String::from_utf8_lossy(&out.stdout).contains("exists")),
    })
}

async fn rename<C: ContainerAccess>(container: &C, from: &str, to: &str, overwrite: FileOverwrite) -> Reply {
    let from = absolute(from)?;
    let to = absolute(to)?;
    if from == "/" || to == "/" {
        return Err(failed(FileOpErrorKind::InvalidPath, "cannot rename the container root"));
    }
    let script = r#"
[ -e "$1" ] || [ -L "$1" ] || exit 44
if [ -d "$2" ] && [ ! -L "$2" ]; then exit 17; fi
if [ "$3" != 1 ] && { [ -e "$2" ] || [ -L "$2" ]; }; then exit 17; fi
mv -f "$1" "$2"
"#;
    let out = exec(
        container,
        script,
        &[&from, &to, if overwrite.overwrites() { "1" } else { "0" }],
        None,
    )
    .await?;
    match out.exit_code {
        0 => Ok(FileOpResponse::Renamed),
        EXIT_NOT_FOUND => Err(FileOpResponse::NotFound { path: from }),
        EXIT_EXISTS => Err(failed(
            FileOpErrorKind::AlreadyExists,
            format!("{to} already exists (a directory target is never replaced)"),
        )),
        _ => Err(script_failure(&out, &to)),
    }
}

async fn archive_put<C: ContainerAccess>(container: &C, path: &str, data: &[u8]) -> Reply {
    let summary = ArchiveSummary::validate(data).map_err(FileOpResponse::from)?;
    let path = absolute(if path.is_empty() { "/" } else { path })?;
    let out = exec(
        container,
        r#"mkdir -p "$1" && tar -xf - -C "$1""#,
        &[&path],
        Some(data.to_vec()),
    )
    .await?;
    if out.exit_code == EXIT_NO_TAR {
        return Err(FileOpResponse::Unsupported {
            message: "the sandbox image has no tar, so an archive cannot be extracted".into(),
        });
    }
    if out.exit_code != 0 {
        return Err(script_failure(&out, &path));
    }
    Ok(FileOpResponse::Extracted {
        entries: summary.entries(),
        bytes: summary.bytes(),
    })
}

pub(crate) mod feed;

#[cfg(test)]
mod tests;
