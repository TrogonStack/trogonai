//! SQLite schema and row access of the attempt ledger.

use std::fs::OpenOptions;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderKind, WorkerId,
};

use crate::subjects::provider_token;
use crate::{AttemptLedgerError, AttemptPhase, CleanupState};

const MAX_RECEIPT_BYTES: usize = 64 * 1024;

pub(crate) fn create_private_file(path: &Path) -> Result<(), AttemptLedgerError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn initialize(
    database: &mut Connection,
    provider: ProviderKind,
    preferred: Option<&WorkerId>,
) -> Result<WorkerId, AttemptLedgerError> {
    database.pragma_update(None, "synchronous", "FULL")?;
    database.busy_timeout(Duration::from_secs(1))?;
    let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS attempts (
            attempt_id TEXT PRIMARY KEY,
            lease_id TEXT NOT NULL UNIQUE,
            phase INTEGER NOT NULL CHECK (phase BETWEEN 0 AND 3),
            intent INTEGER NOT NULL CHECK (intent IN (0, 1)),
            allocation_started INTEGER NOT NULL CHECK (allocation_started IN (0, 1)),
            backend_cleaned INTEGER NOT NULL CHECK (backend_cleaned IN (0, 1)),
            receipt BLOB,
            epoch TEXT
        );
        CREATE INDEX IF NOT EXISTS attempts_by_phase ON attempts(phase);",
    )?;
    let has_epoch: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('attempts') WHERE name='epoch')",
        [],
        |row| row.get(0),
    )?;
    if !has_epoch {
        tx.execute("ALTER TABLE attempts ADD COLUMN epoch TEXT", [])?;
    }
    let version: Option<String> = tx
        .query_row("SELECT value FROM metadata WHERE key = 'version'", [], |row| row.get(0))
        .optional()?;
    let provider_json = serde_json::to_string(&provider)?;
    let worker_id = match version {
        Some(version) => {
            if version != "1" {
                return Err(AttemptLedgerError::FormatMismatch);
            }
            let stored_provider: String =
                tx.query_row("SELECT value FROM metadata WHERE key = 'provider'", [], |row| {
                    row.get(0)
                })?;
            if stored_provider != provider_json {
                return Err(AttemptLedgerError::ProviderMismatch);
            }
            let id: String = tx.query_row("SELECT value FROM metadata WHERE key = 'worker_id'", [], |row| {
                row.get(0)
            })?;
            WorkerId::new(id).map_err(AttemptLedgerError::InvalidWorkerId)?
        }
        None => {
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM attempts", [], |row| row.get(0))?;
            if count != 0 {
                return Err(AttemptLedgerError::MetadataMissing);
            }
            let worker_id = match preferred {
                Some(id) => id.clone(),
                None => WorkerId::new(format!("{}-{}", provider_token(provider), uuid::Uuid::new_v4()))
                    .map_err(AttemptLedgerError::InvalidWorkerId)?,
            };
            tx.execute(
                "INSERT INTO metadata(key,value) VALUES ('version','1'),('provider',?1),('worker_id',?2)",
                params![provider_json, worker_id.as_str()],
            )?;
            worker_id
        }
    };
    if preferred.is_some_and(|id| id != &worker_id) {
        return Err(AttemptLedgerError::WorkerIdentityMismatch);
    }
    tx.commit()?;
    Ok(worker_id)
}

pub(crate) fn write_state(database: &Connection, state: &CleanupState) -> Result<(), AttemptLedgerError> {
    let receipt = state.receipt.as_ref().map(serde_json::to_vec).transpose()?;
    if receipt.as_ref().is_some_and(|bytes| bytes.len() > MAX_RECEIPT_BYTES) {
        return Err(AttemptLedgerError::ReceiptTooLarge);
    }
    let changed = database.execute(
        "INSERT INTO attempts(attempt_id,lease_id,phase,intent,allocation_started,backend_cleaned,receipt,epoch)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
        ON CONFLICT(attempt_id) DO UPDATE SET phase=excluded.phase,intent=excluded.intent,
            allocation_started=excluded.allocation_started,backend_cleaned=excluded.backend_cleaned,receipt=excluded.receipt
        WHERE attempts.lease_id=excluded.lease_id AND attempts.epoch IS excluded.epoch",
        params![
            state.identity.target.attempt_id.as_str(),
            state.identity.lease_id.as_str(),
            state.phase.code(),
            i64::from(state.intent == CancelIntent::Purge),
            state.allocation_started,
            state.backend_cleaned,
            receipt,
            state.identity.target.epoch.map(|epoch| epoch.get().to_string())
        ],
    )?;
    if changed != 1 {
        return Err(AttemptLedgerError::AttemptIdentityReused);
    }
    Ok(())
}

type Row = (String, i64, i64, bool, bool, Option<Vec<u8>>, Option<String>);

pub(crate) fn read_state(
    database: &Connection,
    provider: ProviderKind,
    worker_id: &WorkerId,
    attempt: &DispatchAttemptId,
) -> Result<Option<CleanupState>, AttemptLedgerError> {
    let row: Option<Row> = database
        .query_row(
            "SELECT lease_id,phase,intent,allocation_started,backend_cleaned,receipt,epoch FROM attempts WHERE attempt_id=?1",
            [attempt.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((lease_id, phase, intent, allocation_started, backend_cleaned, receipt, epoch)) = row else {
        return Ok(None);
    };
    let epoch = epoch
        .map(|value| {
            value
                .parse::<u64>()
                .ok()
                .and_then(|value| LeaseEpoch::new(value).ok())
                .ok_or(AttemptLedgerError::InvalidEpoch)
        })
        .transpose()?;
    let intent = match intent {
        0 => CancelIntent::Release,
        1 => CancelIntent::Purge,
        _ => return Err(AttemptLedgerError::InvalidIntent),
    };
    Ok(Some(CleanupState {
        identity: LeaseAttemptIdentity {
            lease_id: LeaseId::new(lease_id),
            provider,
            target: DispatchTarget {
                epoch,
                worker_id: worker_id.clone(),
                attempt_id: attempt.clone(),
            },
        },
        phase: AttemptPhase::from_code(phase)?,
        intent,
        allocation_started,
        backend_cleaned,
        receipt: receipt.map(|bytes| serde_json::from_slice(&bytes)).transpose()?,
    }))
}
