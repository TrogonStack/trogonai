use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use super::{DenialRecord, ReleaseAudit, ReleaseAuditError, ReleaseRecord};

/// An audit trail held in memory, for tests that drive the release core
/// without an audit writer.
#[derive(Clone, Default)]
pub struct RecordingReleaseAudit {
    releases: Arc<Mutex<Vec<ReleaseRecord>>>,
    denials: Arc<Mutex<Vec<DenialRecord>>>,
    refuse_releases: Arc<AtomicBool>,
    refuse_denials: Arc<AtomicBool>,
}

impl RecordingReleaseAudit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes every later release write go unacknowledged.
    pub fn refuse_releases(&self) {
        self.refuse_releases.store(true, Ordering::SeqCst);
    }

    /// Makes every later denial write go unacknowledged.
    pub fn refuse_denials(&self) {
        self.refuse_denials.store(true, Ordering::SeqCst);
    }

    /// The acknowledged release records, oldest first.
    pub fn releases(&self) -> Vec<ReleaseRecord> {
        self.releases.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The acknowledged denial records, oldest first.
    pub fn denials(&self) -> Vec<DenialRecord> {
        self.denials.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl ReleaseAudit for RecordingReleaseAudit {
    async fn record_release(&self, record: &ReleaseRecord) -> Result<(), ReleaseAuditError> {
        if self.refuse_releases.load(Ordering::SeqCst) {
            return Err(ReleaseAuditError::new("release record refused"));
        }
        self.releases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(record.clone());
        Ok(())
    }

    async fn record_denial(&self, record: &DenialRecord) -> Result<(), ReleaseAuditError> {
        if self.refuse_denials.load(Ordering::SeqCst) {
            return Err(ReleaseAuditError::new("denial record refused"));
        }
        self.denials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(record.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
