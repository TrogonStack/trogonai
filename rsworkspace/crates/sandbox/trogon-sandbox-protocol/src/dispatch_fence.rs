//! The worker's record of which tenant and attempt each lease belongs to.
//!
//! The table starts empty. When a journal file is configured every change is appended to it, and the next start
//! loads it back, so a restarted worker keeps refusing a lease id that is claimed for another tenant and keeps
//! refusing a dispatch for an attempt it has already torn down.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{DispatchAttemptId, DispatchTarget, LeaseEpoch, LeaseId, TenantId, WorkerId};

use crate::{DispatchAuthError, EpochMode, FreshLaunchFenceError};

const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
const CAPACITY: usize = 100_000;
/// Journal lines beyond the live entries that trigger a rewrite on open.
const COMPACT_SLACK: usize = 1024;

struct Entry {
    tenant: String,
    worker: Option<String>,
    attempt: Option<String>,
    /// Highest lease epoch seen for the lease; 0 when none was ever signed.
    epoch: u64,
    cleaned: bool,
    seen: Instant,
    recovered: bool,
}

/// One journal line. Later lines for a lease replace earlier ones.
#[derive(Serialize, Deserialize)]
struct Line {
    l: String,
    #[serde(default)]
    t: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    w: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    a: Option<String>,
    #[serde(default)]
    c: bool,
    /// Highest lease epoch seen; absent in journals from before epochs.
    #[serde(default, skip_serializing_if = "is_zero")]
    e: u64,
    /// Tombstone: the lease was forgotten.
    #[serde(default)]
    x: bool,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

pub(crate) struct DispatchFence {
    entries: HashMap<String, Entry>,
    journal: Option<Journal>,
    recovery_trusted: bool,
}

#[cfg(unix)]
#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl FileIdentity {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: std::os::unix::fs::MetadataExt::dev(metadata),
            inode: std::os::unix::fs::MetadataExt::ino(metadata),
        }
    }
}

struct Journal {
    file: File,
    #[cfg(unix)]
    path: PathBuf,
    minimum_synced_length: u64,
    failed: bool,
    #[cfg(unix)]
    identity: Option<FileIdentity>,
}

impl Journal {
    fn new(file: File, _path: &Path) -> Self {
        let metadata = file.metadata();
        let synced = file.sync_data().is_ok();
        Self {
            #[cfg(unix)]
            identity: metadata.as_ref().ok().map(FileIdentity::of),
            minimum_synced_length: metadata.as_ref().map_or(0, std::fs::Metadata::len),
            failed: !synced || metadata.is_err(),
            file,
            #[cfg(unix)]
            path: _path.to_owned(),
        }
    }

    fn append(&mut self, text: &str, sync: impl FnOnce(&File) -> std::io::Result<()>) -> std::io::Result<()> {
        writeln!(self.file, "{text}")?;
        sync(&self.file)?;
        self.minimum_synced_length = self.minimum_synced_length.max(self.file.metadata()?.len());
        Ok(())
    }

    fn health(&self) -> Result<(), FreshLaunchFenceError> {
        if self.failed {
            return Err(FreshLaunchFenceError::JournalIoFailed);
        }
        #[cfg(not(unix))]
        {
            Err(FreshLaunchFenceError::UnsupportedPlatform)
        }
        #[cfg(unix)]
        {
            let original = self.identity.ok_or(FreshLaunchFenceError::JournalIdentityUnavailable)?;
            let opened = self
                .file
                .metadata()
                .map_err(|_| FreshLaunchFenceError::JournalIdentityUnavailable)?;
            let canonical =
                std::fs::symlink_metadata(&self.path).map_err(|_| FreshLaunchFenceError::JournalIdentityUnavailable)?;
            if !opened.is_file() || !canonical.is_file() {
                return Err(FreshLaunchFenceError::JournalIdentityUnavailable);
            }
            if FileIdentity::of(&opened) != original
                || FileIdentity::of(&canonical) != original
                || opened.len() < self.minimum_synced_length
                || canonical.len() < self.minimum_synced_length
            {
                return Err(FreshLaunchFenceError::JournalChanged);
            }
            Ok(())
        }
    }
}

fn open_private(path: &Path, append: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true);
    if append {
        options.append(true);
    } else {
        options.write(true).truncate(true);
    }
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

impl DispatchFence {
    pub(crate) fn memory() -> Self {
        Self {
            entries: HashMap::new(),
            journal: None,
            recovery_trusted: false,
        }
    }

    /// Loads the journal at `path` (creating it when absent) and keeps appending to it. A torn last line from a
    /// crash is ignored.
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut entries = HashMap::new();
        let mut lines = 0usize;
        let mut recovery_trusted = true;
        if path.exists() {
            let mut reader = BufReader::new(File::open(path)?);
            loop {
                let mut text = String::new();
                match reader.read_line(&mut text) {
                    Ok(0) => break,
                    Ok(_) => {
                        if !text.ends_with('\n') {
                            recovery_trusted = false;
                        }
                    }
                    Err(error) => {
                        recovery_trusted = false;
                        if error.kind() == std::io::ErrorKind::InvalidData {
                            continue;
                        }
                        break;
                    }
                }
                let Ok(line) = serde_json::from_str::<Line>(&text) else {
                    recovery_trusted = false;
                    continue;
                };
                lines += 1;
                if line.x {
                    entries.remove(&line.l);
                } else {
                    entries.insert(
                        line.l,
                        Entry {
                            tenant: line.t,
                            worker: line.w,
                            attempt: line.a,
                            epoch: line.e,
                            cleaned: line.c,
                            seen: Instant::now(),
                            recovered: true,
                        },
                    );
                }
            }
        }
        let mut fence = Self {
            entries,
            journal: None,
            recovery_trusted,
        };
        if recovery_trusted && lines > fence.entries.len() * 2 + COMPACT_SLACK {
            fence.rewrite(path)?;
        }
        fence.journal = Some(Journal::new(open_private(path, true)?, path));
        Ok(fence)
    }

    pub(crate) fn fresh_launch_health(&self) -> Result<(), FreshLaunchFenceError> {
        let journal = self.journal.as_ref().ok_or(FreshLaunchFenceError::JournalUnavailable)?;
        if !self.recovery_trusted {
            return Err(FreshLaunchFenceError::UntrustedJournal);
        }
        journal.health()
    }

    fn rewrite(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        {
            let mut file = open_private(&tmp, false)?;
            for (lease, entry) in &self.entries {
                writeln!(file, "{}", serde_json::to_string(&Self::line(lease, entry))?)?;
            }
            file.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    }

    fn line(lease: &str, entry: &Entry) -> Line {
        Line {
            l: lease.to_owned(),
            t: entry.tenant.clone(),
            w: entry.worker.clone(),
            a: entry.attempt.clone(),
            c: entry.cleaned,
            e: entry.epoch,
            x: false,
        }
    }

    fn append(&mut self, line: &Line) {
        self.append_with_sync(line, File::sync_data);
    }

    fn append_with_sync(&mut self, line: &Line, sync: impl FnOnce(&File) -> std::io::Result<()>) {
        let Some(journal) = self.journal.as_mut() else {
            return;
        };
        let written = serde_json::to_string(line)
            .map_err(std::io::Error::from)
            .and_then(|text| journal.append(&text, sync));
        if let Err(error) = written {
            journal.failed = true;
            tracing::warn!(%error, "dispatch fence journal write failed");
        }
    }

    fn persist(&mut self, lease: &str) {
        if let Some(entry) = self.entries.get(lease) {
            let line = Self::line(lease, entry);
            self.append(&line);
        }
    }

    fn make_room(&mut self) {
        if self.entries.len() < CAPACITY {
            return;
        }
        self.entries.retain(|_, entry| entry.seen.elapsed() < RETENTION);
        if self.entries.len() >= CAPACITY
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.seen)
                .map(|(lease, _)| lease.clone())
        {
            self.entries.remove(&oldest);
        }
    }

    /// Compares an incoming epoch with the held one. A lower epoch is logged under `warn` and refused under
    /// `require`; an absent, equal or higher epoch always passes.
    fn epoch_ok(
        lease: &str,
        held: u64,
        incoming: Option<LeaseEpoch>,
        mode: EpochMode,
    ) -> Result<(), DispatchAuthError> {
        let Some(seen) = incoming.map(LeaseEpoch::get) else {
            return Ok(());
        };
        if mode == EpochMode::Off || seen >= held {
            return Ok(());
        }
        tracing::warn!(
            lease_id = lease,
            held_epoch = held,
            seen_epoch = seen,
            enforced = mode == EpochMode::Require,
            "request carries an older lease epoch than the one held"
        );
        if mode == EpochMode::Require {
            Err(DispatchAuthError::StaleEpoch)
        } else {
            Ok(())
        }
    }

    /// Raises the held epoch to `incoming`; true when it changed.
    fn adopt(entry: &mut Entry, incoming: Option<LeaseEpoch>) -> bool {
        match incoming.map(LeaseEpoch::get) {
            Some(seen) if seen > entry.epoch => {
                entry.epoch = seen;
                entry.recovered = false;
                true
            }
            _ => false,
        }
    }

    fn adopt_worker(entry: &mut Entry, worker: Option<&WorkerId>) -> bool {
        let Some(worker) = worker else {
            return false;
        };
        if entry.worker.as_deref() == Some(worker.as_str()) {
            return false;
        }
        entry.worker = Some(worker.as_str().to_owned());
        entry.recovered = false;
        true
    }

    /// The tenant an independent, trusted journal recorded for exactly this assignment, if any.
    pub(crate) fn recovered_dispatch_tenant(&self, lease: &LeaseId, target: &DispatchTarget) -> Option<TenantId> {
        self.journal.as_ref()?;
        if !self.recovery_trusted {
            return None;
        }
        let epoch = target.epoch?;
        let entry = self.entries.get(lease.as_str())?;
        if !(entry.recovered
            && !entry.cleaned
            && entry.worker.as_deref() == Some(target.worker_id.as_str())
            && entry.attempt.as_deref() == Some(target.attempt_id.as_str())
            && entry.epoch == epoch.get())
        {
            return None;
        }
        TenantId::new(entry.tenant.clone()).ok()
    }

    fn tenant_ok(entry: &Entry, tenant: &TenantId) -> Result<(), DispatchAuthError> {
        if entry.tenant == tenant.as_str() {
            Ok(())
        } else {
            Err(DispatchAuthError::LeaseTenantConflict)
        }
    }

    /// A lease dispatch: first sight records the tenant and attempt. The same attempt may be re-sent until it is
    /// cleaned up; another attempt is refused while one is live, and an attempt already cleaned up is never
    /// accepted again.
    pub(crate) fn admit_dispatch(
        &mut self,
        lease: &LeaseId,
        tenant: &TenantId,
        worker: Option<&WorkerId>,
        attempt: Option<&DispatchAttemptId>,
        epoch: Option<LeaseEpoch>,
        mode: EpochMode,
    ) -> Result<(), DispatchAuthError> {
        let lease = lease.as_str();
        if let Some(entry) = self.entries.get_mut(lease) {
            Self::tenant_ok(entry, tenant)?;
            if !entry.cleaned
                && let (Some(held), Some(asked)) = (entry.worker.as_deref(), worker)
                && held != asked.as_str()
            {
                return Err(DispatchAuthError::WrongWorker {
                    expected: held.to_owned(),
                    found: asked.as_str().to_owned(),
                });
            }
            Self::epoch_ok(lease, entry.epoch, epoch, mode)?;
            entry.seen = Instant::now();
            let raised = Self::adopt(entry, epoch);
            let Some(attempt) = attempt else {
                let worker_changed = Self::adopt_worker(entry, worker);
                if raised || worker_changed {
                    self.persist(lease);
                }
                return Ok(());
            };
            let changed = match entry.attempt.as_deref() {
                Some(held) if held == attempt.as_str() => {
                    if entry.cleaned {
                        return Err(DispatchAuthError::StaleAttempt);
                    }
                    false
                }
                Some(_) if !entry.cleaned => return Err(DispatchAuthError::StaleAttempt),
                _ => {
                    entry.attempt = Some(attempt.as_str().to_owned());
                    entry.worker = worker.map(|worker| worker.as_str().to_owned());
                    entry.cleaned = false;
                    entry.recovered = false;
                    true
                }
            };
            let worker_changed = Self::adopt_worker(entry, worker);
            if changed || raised || worker_changed {
                self.persist(lease);
            }
            return Ok(());
        }
        self.make_room();
        self.entries.insert(
            lease.to_owned(),
            Entry {
                tenant: tenant.as_str().to_owned(),
                worker: worker.map(|worker| worker.as_str().to_owned()),
                attempt: attempt.map(|attempt| attempt.as_str().to_owned()),
                epoch: epoch.map_or(0, LeaseEpoch::get),
                cleaned: false,
                seen: Instant::now(),
                recovered: false,
            },
        );
        self.persist(lease);
        Ok(())
    }

    /// A cleanup request: the attempt is marked cleaned, so neither its dispatch nor its verbs are accepted
    /// afterwards. Repeating a cleanup is fine; cleaning up an attempt other than the live one is refused.
    pub(crate) fn admit_cleanup(
        &mut self,
        lease: &LeaseId,
        tenant: &TenantId,
        attempt: Option<&DispatchAttemptId>,
        epoch: Option<LeaseEpoch>,
        mode: EpochMode,
    ) -> Result<(), DispatchAuthError> {
        let lease = lease.as_str();
        let attempt = attempt.map(DispatchAttemptId::as_str);
        if let Some(entry) = self.entries.get_mut(lease) {
            Self::tenant_ok(entry, tenant)?;
            Self::epoch_ok(lease, entry.epoch, epoch, mode)?;
            if let (Some(held), Some(asked)) = (entry.attempt.as_deref(), attempt)
                && held != asked
                && !entry.cleaned
            {
                return Err(DispatchAuthError::StaleAttempt);
            }
            let unchanged = entry.cleaned
                && (attempt.is_none() || entry.attempt.as_deref() == attempt)
                && epoch.is_none_or(|seen| seen.get() <= entry.epoch);
            Self::adopt(entry, epoch);
            entry.cleaned = true;
            if let Some(attempt) = attempt {
                entry.attempt = Some(attempt.to_owned());
            }
            entry.seen = Instant::now();
            if !unchanged {
                entry.recovered = false;
                self.persist(lease);
            }
            return Ok(());
        }
        self.make_room();
        self.entries.insert(
            lease.to_owned(),
            Entry {
                tenant: tenant.as_str().to_owned(),
                worker: None,
                attempt: attempt.map(str::to_owned),
                epoch: epoch.map_or(0, LeaseEpoch::get),
                cleaned: true,
                seen: Instant::now(),
                recovered: false,
            },
        );
        self.persist(lease);
        Ok(())
    }

    /// Requires an exact, live, existing fence entry for a command readiness probe.
    pub(crate) fn check_command_readiness(
        &self,
        lease: &LeaseId,
        tenant: &TenantId,
        worker: &WorkerId,
        attempt: &DispatchAttemptId,
        epoch: LeaseEpoch,
    ) -> Result<(), DispatchAuthError> {
        let entry = self
            .entries
            .get(lease.as_str())
            .ok_or(DispatchAuthError::LeaseMismatch)?;
        Self::tenant_ok(entry, tenant)?;
        if entry.worker.as_deref() != Some(worker.as_str()) {
            return Err(DispatchAuthError::WrongWorker {
                expected: entry.worker.clone().unwrap_or_default(),
                found: worker.as_str().to_owned(),
            });
        }
        if entry.cleaned || entry.attempt.as_deref() != Some(attempt.as_str()) {
            return Err(DispatchAuthError::StaleAttempt);
        }
        if entry.epoch != epoch.get() {
            return Err(DispatchAuthError::StaleEpoch);
        }
        Ok(())
    }

    /// A per-lease verb. Nothing is recorded: a broadcast verb reaches workers that never held the lease.
    pub(crate) fn check_verb(
        &self,
        lease: &LeaseId,
        tenant: &TenantId,
        attempt: Option<&DispatchAttemptId>,
        epoch: Option<LeaseEpoch>,
        mode: EpochMode,
    ) -> Result<(), DispatchAuthError> {
        let Some(entry) = self.entries.get(lease.as_str()) else {
            return Ok(());
        };
        Self::tenant_ok(entry, tenant)?;
        Self::epoch_ok(lease.as_str(), entry.epoch, epoch, mode)?;
        match (entry.attempt.as_deref(), attempt) {
            (Some(held), Some(asked)) if held != asked.as_str() => Err(DispatchAuthError::StaleAttempt),
            (Some(_), Some(_)) if entry.cleaned => Err(DispatchAuthError::StaleAttempt),
            _ => Ok(()),
        }
    }

    /// Forgets a lease, journaling a tombstone.
    pub(crate) fn forget(&mut self, lease: &LeaseId) {
        if self.entries.remove(lease.as_str()).is_some() {
            self.append(&Line {
                l: lease.as_str().to_owned(),
                t: String::new(),
                w: None,
                a: None,
                c: false,
                e: 0,
                x: true,
            });
        }
    }
}

#[cfg(test)]
#[path = "dispatch_fence_health_tests.rs"]
mod health_tests;
