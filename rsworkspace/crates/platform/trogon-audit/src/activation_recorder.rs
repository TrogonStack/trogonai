use crate::audit_sink::AuditSink;
use crate::audit_unavailable_error::AuditUnavailableError;
use crate::audit_writer::AuditWriter;
use crate::credential_activation_facts::{ActivationScopeError, CredentialActivationFacts};
use crate::record_id::RecordId;
use crate::record_scope::RecordScope;

pub struct ActivationRecorder<S> {
    writer: AuditWriter<S>,
    scope: RecordScope,
}

#[derive(Debug, thiserror::Error)]
pub enum ActivationRecorderError {
    #[error("activation binding does not match the audit scope")]
    Scope(#[from] ActivationScopeError),
    #[error("activation audit is unavailable")]
    Unavailable(#[from] AuditUnavailableError),
    #[error("activation audit was not stored")]
    NotStored,
}

impl<S: AuditSink> ActivationRecorder<S> {
    pub fn new(writer: AuditWriter<S>, scope: RecordScope) -> Self {
        Self { writer, scope }
    }

    pub async fn record(&self, _facts: CredentialActivationFacts) -> Result<RecordId, ActivationRecorderError> {
        let _ = (&self.writer, &self.scope);
        Err(ActivationRecorderError::NotStored)
    }
}

#[cfg(test)]
mod tests;
