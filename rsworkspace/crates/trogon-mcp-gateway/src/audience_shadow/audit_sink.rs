//! Audit sink boundary for `mcp.audit.gateway.aud_mismatch` envelopes (no concrete JetStream writer).

use std::sync::{Mutex, MutexGuard};

/// JetStream subject for gateway ingress audience mismatch (shadow and enforce telemetry).
pub const AUD_MISMATCH_AUDIT_SUBJECT: &str = "mcp.audit.gateway.aud_mismatch";

/// Payload emitted when inbound mesh `aud` does not include the expected backend/gateway URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudMismatchEnvelope {
    pub tenant_id: String,
    pub caller_sub: String,
    pub request_id: String,
    pub expected_aud: String,
    pub observed_aud: Vec<String>,
    pub ts_unix_ms: u64,
}

/// Pluggable audit writer; production wiring adapts `audit::AuditPublisher` in a follow-up PR.
pub trait AudienceAuditSink {
    fn emit_mismatch(&self, envelope: AudMismatchEnvelope);
}

/// In-memory sink for unit and integration tests.
#[derive(Debug, Default)]
pub struct RecordingAuditSink {
    records: Mutex<Vec<AudMismatchEnvelope>>,
}

impl RecordingAuditSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lock_records(&self) -> MutexGuard<'_, Vec<AudMismatchEnvelope>> {
        self.records.lock().expect("recording audit sink mutex")
    }

    #[must_use]
    pub fn drain(&self) -> Vec<AudMismatchEnvelope> {
        let mut guard = self.lock_records();
        std::mem::take(&mut *guard)
    }
}

impl AudienceAuditSink for RecordingAuditSink {
    fn emit_mismatch(&self, envelope: AudMismatchEnvelope) {
        self.lock_records().push(envelope);
    }
}
