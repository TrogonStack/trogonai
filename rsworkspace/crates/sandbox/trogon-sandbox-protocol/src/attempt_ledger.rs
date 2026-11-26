use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions, TryLockError};
use std::future::Future;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use rusqlite::{Connection, TransactionBehavior};
use serde::Deserialize;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, LeaseAttemptIdentity, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderKind, ProviderLease, WorkerId,
};

use crate::attempt_ledger_storage::{create_private_file, initialize, read_state, write_state};
use crate::{AttemptLedgerError, AttemptPhase, CleanupState};

const MAX_LEGACY_BYTES: u64 = 64 * 1024 * 1024;
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

#[derive(Deserialize)]
struct LegacyJournal {
    version: u32,
    provider: ProviderKind,
    worker_id: WorkerId,
    attempts: BTreeMap<DispatchAttemptId, CleanupState>,
}

/// What a reopened ledger does with attempts that were in flight when the previous process stopped.
enum ReopenPolicy {
    /// Every unfinished attempt is queued for cleanup.
    Cleanup,
    /// A ready attempt whose receipt is coherent stays ready so its backend resource can be adopted.
    AdoptReady,
}

/// The durable, single-owner record of one worker's dispatch attempts.
///
/// It lives in a SQLite database guarded by a lock file, so a second worker on the same path is refused and an
/// attempt that was in flight at a crash is found again on the next start.
pub struct AttemptLedger {
    path: PathBuf,
    provider: ProviderKind,
    worker_id: WorkerId,
    database: Mutex<Connection>,
    gates: std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>,
    #[cfg(unix)]
    file_identity: (u64, u64),
    lock: File,
}

impl Drop for AttemptLedger {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

impl AttemptLedger {
    /// Opens the ledger, keeping a ready attempt with a coherent receipt ready so its backend resource can be
    /// adopted, and queueing every other unfinished attempt for cleanup.
    pub fn open_for_adoption(
        path: impl Into<PathBuf>,
        provider: ProviderKind,
        preferred_worker_id: Option<WorkerId>,
    ) -> Result<Self, AttemptLedgerError> {
        Self::open_with_policy(path, provider, preferred_worker_id, ReopenPolicy::AdoptReady)
    }

    /// Opens the ledger, queueing every unfinished attempt for cleanup.
    pub fn open(
        path: impl Into<PathBuf>,
        provider: ProviderKind,
        preferred_worker_id: Option<WorkerId>,
    ) -> Result<Self, AttemptLedgerError> {
        Self::open_with_policy(path, provider, preferred_worker_id, ReopenPolicy::Cleanup)
    }

    fn open_with_policy(
        path: impl Into<PathBuf>,
        provider: ProviderKind,
        preferred_worker_id: Option<WorkerId>,
        policy: ReopenPolicy,
    ) -> Result<Self, AttemptLedgerError> {
        let path = std::path::absolute(path.into())?;
        let parent = path.parent().ok_or(AttemptLedgerError::NoParent)?.to_owned();
        std::fs::create_dir_all(&parent)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let lock = options.open(path.with_extension("lock"))?;
        lock.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => AttemptLedgerError::Locked,
            TryLockError::Error(error) => AttemptLedgerError::Io(error),
        })?;
        if let Some(legacy) = Self::read_legacy(&path)? {
            Self::migrate_legacy(&path, &parent, provider, preferred_worker_id.as_ref(), legacy)?;
        }
        create_private_file(&path)?;
        let mut database = Connection::open(&path)?;
        let worker_id = initialize(&mut database, provider, preferred_worker_id.as_ref())?;
        database.pragma_update(None, "journal_mode", "WAL")?;
        database.pragma_update(None, "synchronous", "FULL")?;
        #[cfg(unix)]
        let file_identity = {
            let metadata = std::fs::metadata(&path)?;
            (
                std::os::unix::fs::MetadataExt::dev(&metadata),
                std::os::unix::fs::MetadataExt::ino(&metadata),
            )
        };
        Self::apply_policy(&mut database, provider, &worker_id, &policy)?;
        File::open(&parent)?.sync_all()?;
        Ok(Self {
            path,
            provider,
            worker_id,
            database: Mutex::new(database),
            gates: std::sync::Mutex::new(HashMap::new()),
            #[cfg(unix)]
            file_identity,
            lock,
        })
    }

    /// The journal an earlier version wrote in place of the database, when the path holds one.
    fn read_legacy(path: &std::path::Path) -> Result<Option<LegacyJournal>, AttemptLedgerError> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut prefix = [0; 16];
        let length = file.read(&mut prefix)?;
        if length == 0 || &prefix == SQLITE_MAGIC {
            return Ok(None);
        }
        if file.metadata()?.len() > MAX_LEGACY_BYTES {
            return Err(AttemptLedgerError::LegacyTooLarge);
        }
        serde_json::from_slice::<LegacyJournal>(&std::fs::read(path)?)
            .map(Some)
            .map_err(AttemptLedgerError::LegacyInvalid)
    }

    fn migrate_legacy(
        path: &std::path::Path,
        parent: &std::path::Path,
        provider: ProviderKind,
        preferred: Option<&WorkerId>,
        legacy: LegacyJournal,
    ) -> Result<(), AttemptLedgerError> {
        if !(legacy.version == 1 && legacy.provider == provider && preferred.is_none_or(|id| id == &legacy.worker_id)) {
            return Err(AttemptLedgerError::LegacyIdentityMismatch);
        }
        let pending = path.with_extension(format!("migrating-{}", uuid::Uuid::new_v4()));
        let migrated = Self::write_migrated(&pending, path, parent, provider, legacy);
        if migrated.is_err() {
            let _ = std::fs::remove_file(&pending);
        }
        migrated
    }

    fn write_migrated(
        pending: &std::path::Path,
        path: &std::path::Path,
        parent: &std::path::Path,
        provider: ProviderKind,
        legacy: LegacyJournal,
    ) -> Result<(), AttemptLedgerError> {
        create_private_file(pending)?;
        let mut database = Connection::open(pending)?;
        initialize(&mut database, provider, Some(&legacy.worker_id))?;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (attempt, mut state) in legacy.attempts {
            if !(state.identity.provider == provider
                && state.identity.target.worker_id == legacy.worker_id
                && state.identity.target.attempt_id == attempt)
            {
                return Err(AttemptLedgerError::LegacyAttemptMismatch);
            }
            if state.phase == AttemptPhase::Cleaned {
                state.backend_cleaned = true;
                state.receipt = None;
            }
            write_state(&tx, &state)?;
        }
        tx.commit()?;
        database.close().map_err(|(_, error)| error)?;
        File::open(pending)?.sync_all()?;
        std::fs::rename(pending, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }

    fn apply_policy(
        database: &mut Connection,
        provider: ProviderKind,
        worker_id: &WorkerId,
        policy: &ReopenPolicy,
    ) -> Result<(), AttemptLedgerError> {
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match policy {
            ReopenPolicy::Cleanup => {
                tx.execute("UPDATE attempts SET phase=2 WHERE phase IN (0,1)", [])?;
            }
            ReopenPolicy::AdoptReady => {
                tx.execute("UPDATE attempts SET phase=2 WHERE phase=0", [])?;
                let attempts = {
                    let mut query = tx.prepare("SELECT attempt_id FROM attempts WHERE phase=1")?;
                    query
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                for attempt in attempts {
                    let attempt = DispatchAttemptId::new(attempt).map_err(AttemptLedgerError::InvalidAttemptId)?;
                    let state = read_state(&tx, provider, worker_id, &attempt)?
                        .ok_or(AttemptLedgerError::RecoveredAttemptDisappeared)?;
                    let coherent = state.allocation_started
                        && !state.backend_cleaned
                        && state.intent == CancelIntent::Release
                        && state.receipt.as_ref().is_some_and(|receipt| {
                            !receipt.sandbox_id.as_str().trim().is_empty()
                                && receipt.worker_id.as_ref() == Some(&state.identity.target.worker_id)
                                && receipt.attempt_id.as_ref() == Some(&state.identity.target.attempt_id)
                                && receipt.epoch == state.identity.target.epoch
                        });
                    if !coherent {
                        tx.execute("UPDATE attempts SET phase=2 WHERE attempt_id=?1", [attempt.as_str()])?;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The worker identity the ledger belongs to.
    pub fn worker_id(&self) -> &WorkerId {
        &self.worker_id
    }

    fn check_backing_file(&self) -> Result<(), AttemptLedgerError> {
        let metadata = std::fs::metadata(&self.path).map_err(AttemptLedgerError::PathUnavailable)?;
        if !metadata.is_file() {
            return Err(AttemptLedgerError::PathNotFile);
        }
        #[cfg(unix)]
        if (
            std::os::unix::fs::MetadataExt::dev(&metadata),
            std::os::unix::fs::MetadataExt::ino(&metadata),
        ) != self.file_identity
        {
            return Err(AttemptLedgerError::PathReplaced);
        }
        Ok(())
    }

    fn read(
        &self,
        database: &Connection,
        attempt: &DispatchAttemptId,
    ) -> Result<Option<CleanupState>, AttemptLedgerError> {
        read_state(database, self.provider, &self.worker_id, attempt)
    }

    /// Whether any attempt, in any phase, belongs to `lease`.
    pub async fn has_lease(&self, lease: &LeaseId) -> Result<bool, AttemptLedgerError> {
        self.check_backing_file()?;
        let database = self.database.lock().await;
        Ok(database.query_row(
            "SELECT EXISTS(SELECT 1 FROM attempts WHERE lease_id=?1)",
            [lease.as_str()],
            |row| row.get(0),
        )?)
    }

    /// Every attempt that is not yet cleaned, ordered by attempt id.
    pub async fn try_states(&self) -> Result<Vec<CleanupState>, AttemptLedgerError> {
        self.check_backing_file()?;
        let database = self.database.lock().await;
        let mut query =
            database.prepare("SELECT attempt_id FROM attempts WHERE phase IN (0,1,2) ORDER BY attempt_id")?;
        let attempts = query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        attempts
            .into_iter()
            .map(|attempt| {
                let attempt = DispatchAttemptId::new(attempt).map_err(AttemptLedgerError::InvalidAttemptId)?;
                self.read(&database, &attempt)?
                    .ok_or(AttemptLedgerError::AttemptDisappeared)
            })
            .collect()
    }

    /// The record of exactly `expected`, or `None` when the ledger holds no attempt with that whole identity.
    pub(crate) async fn authority_state(
        &self,
        expected: &LeaseAttemptIdentity,
    ) -> Result<Option<CleanupState>, AttemptLedgerError> {
        self.check_backing_file()?;
        if expected.provider != self.provider || expected.target.worker_id != self.worker_id {
            return Ok(None);
        }
        let database = self.database.lock().await;
        Ok(self
            .read(&database, &expected.target.attempt_id)?
            .filter(|state| state.identity == *expected))
    }

    fn validate(&self, identity: &LeaseAttemptIdentity) -> Result<(), AttemptLedgerError> {
        if identity.provider != self.provider || identity.target.worker_id != self.worker_id {
            return Err(AttemptLedgerError::ForeignIdentity);
        }
        if identity.lease_id.as_str().is_empty() {
            return Err(AttemptLedgerError::EmptyLease);
        }
        Ok(())
    }

    fn gate(&self, identity: &LeaseAttemptIdentity) -> Arc<Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(|error| error.into_inner());
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(identity.lease_id.as_str()).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(identity.lease_id.as_str().to_owned(), Arc::downgrade(&gate));
        gate
    }

    /// Records the backend resource an in-flight allocation produced, so a crash before the allocation returns
    /// still leaves enough to tear it down.
    pub async fn record_backend_receipt(
        &self,
        identity: &LeaseAttemptIdentity,
        mut receipt: ProviderLease,
    ) -> Result<(), AttemptLedgerError> {
        self.validate(identity)?;
        self.check_backing_file()?;
        let mut database = self.database.lock().await;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut state = self
            .read(&tx, &identity.target.attempt_id)?
            .ok_or(AttemptLedgerError::ReceiptWithoutAttempt)?;
        if !(state.identity == *identity
            && state.allocation_started
            && !state.backend_cleaned
            && state.phase != AttemptPhase::Cleaned)
        {
            return Err(AttemptLedgerError::ReceiptMismatch);
        }
        if state
            .receipt
            .as_ref()
            .is_some_and(|existing| existing.sandbox_id != receipt.sandbox_id)
        {
            return Err(AttemptLedgerError::BackendIdentityChanged);
        }
        receipt.worker_id = Some(self.worker_id.clone());
        receipt.attempt_id = Some(identity.target.attempt_id.clone());
        state.receipt = Some(receipt);
        write_state(&tx, &state)?;
        tx.commit()?;
        Ok(())
    }

    /// Runs `allocate` for a new attempt, durably recording the start before and the receipt after. An attempt that
    /// was already allocated returns its receipt without running `allocate` again.
    pub async fn dispatch<F, Fut, E>(
        &self,
        identity: &LeaseAttemptIdentity,
        allocate: F,
    ) -> Result<ProviderLease, AttemptLedgerError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<ProviderLease, E>>,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.validate(identity)?;
        let _gate = self.gate(identity).lock_owned().await;
        {
            self.check_backing_file()?;
            let mut database = self.database.lock().await;
            let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = self.read(&tx, &identity.target.attempt_id)? {
                if existing.identity != *identity {
                    return Err(AttemptLedgerError::AttemptIdentityReused);
                }
                return match existing.phase {
                    AttemptPhase::Ready => existing.receipt.ok_or(AttemptLedgerError::ReadyWithoutReceipt),
                    AttemptPhase::Starting => Err(AttemptLedgerError::AlreadyStarted),
                    AttemptPhase::CleanupRequested | AttemptPhase::Cleaned => Err(AttemptLedgerError::Cancelled),
                };
            }
            write_state(
                &tx,
                &CleanupState {
                    identity: identity.clone(),
                    allocation_started: true,
                    receipt: None,
                    phase: AttemptPhase::Starting,
                    intent: CancelIntent::Release,
                    backend_cleaned: false,
                },
            )?;
            tx.commit()?;
        }
        let mut receipt = allocate()
            .await
            .map_err(|error| AttemptLedgerError::Allocation(error.into()))?;
        receipt.worker_id = Some(self.worker_id.clone());
        receipt.attempt_id = Some(identity.target.attempt_id.clone());
        self.check_backing_file()?;
        let mut database = self.database.lock().await;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut state = self
            .read(&tx, &identity.target.attempt_id)?
            .ok_or(AttemptLedgerError::AttemptDisappeared)?;
        if state
            .receipt
            .as_ref()
            .is_some_and(|existing| existing.sandbox_id != receipt.sandbox_id)
        {
            return Err(AttemptLedgerError::BackendIdentityChanged);
        }
        state.receipt = Some(receipt.clone());
        let cancelled = state.phase == AttemptPhase::CleanupRequested;
        if !cancelled {
            state.phase = AttemptPhase::Ready;
        }
        write_state(&tx, &state)?;
        tx.commit()?;
        if cancelled {
            return Err(AttemptLedgerError::CancelledDuringAllocation);
        }
        Ok(receipt)
    }

    /// Marks the attempt for cleanup, runs `teardown` on its record when a backend allocation was started, and
    /// durably records the result. A repeated cleanup of a cleaned attempt succeeds without running `teardown`.
    pub async fn cleanup<F, Fut, E>(&self, request: &ProviderCleanupRequest, teardown: F) -> ProviderCleanupResponse
    where
        F: FnOnce(CleanupState) -> Fut,
        Fut: Future<Output = Result<(), E>>,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        match self.cleanup_inner(request, teardown).await {
            Ok(()) => ProviderCleanupResponse::Cleaned {
                identity: request.identity.clone(),
                intent: request.intent,
            },
            Err(error) => ProviderCleanupResponse::Failed {
                identity: request.identity.clone(),
                message: error.to_string(),
            },
        }
    }

    async fn cleanup_inner<F, Fut, E>(
        &self,
        request: &ProviderCleanupRequest,
        teardown: F,
    ) -> Result<(), AttemptLedgerError>
    where
        F: FnOnce(CleanupState) -> Fut,
        Fut: Future<Output = Result<(), E>>,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let identity = &request.identity;
        self.validate(identity)?;
        {
            self.check_backing_file()?;
            let mut database = self.database.lock().await;
            let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut state = match self.read(&tx, &identity.target.attempt_id)? {
                Some(existing) => {
                    if existing.identity != *identity {
                        return Err(AttemptLedgerError::CleanupIdentityReused);
                    }
                    if existing.phase == AttemptPhase::Cleaned
                        && (existing.intent == CancelIntent::Purge || request.intent == CancelIntent::Release)
                    {
                        return Ok(());
                    }
                    existing
                }
                None => CleanupState {
                    identity: identity.clone(),
                    allocation_started: false,
                    receipt: None,
                    phase: AttemptPhase::CleanupRequested,
                    intent: request.intent,
                    backend_cleaned: false,
                },
            };
            if request.intent == CancelIntent::Purge {
                state.intent = CancelIntent::Purge;
            }
            state.phase = AttemptPhase::CleanupRequested;
            write_state(&tx, &state)?;
            tx.commit()?;
        }
        let _gate = self.gate(identity).lock_owned().await;
        let state = {
            let database = self.database.lock().await;
            self.read(&database, &identity.target.attempt_id)?
                .ok_or(AttemptLedgerError::CleanupAttemptDisappeared)?
        };
        if state.phase == AttemptPhase::Cleaned {
            return Ok(());
        }
        let completed_intent = state.intent;
        if state.allocation_started {
            teardown(state)
                .await
                .map_err(|error| AttemptLedgerError::Teardown(error.into()))?;
        }
        self.check_backing_file()?;
        let mut database = self.database.lock().await;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut state = self
            .read(&tx, &identity.target.attempt_id)?
            .ok_or(AttemptLedgerError::CleanupAttemptDisappeared)?;
        if state.intent != completed_intent {
            return Err(AttemptLedgerError::IntentEscalated);
        }
        state.phase = AttemptPhase::Cleaned;
        state.backend_cleaned = true;
        state.receipt = None;
        write_state(&tx, &state)?;
        tx.commit()?;
        Ok(())
    }
}
