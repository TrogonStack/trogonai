use crate::audit_sink::AuditSink;
use crate::audit_unavailable_error::AuditUnavailableError;
use crate::audit_writer::AuditWriter;
use crate::credential_activation_facts::{ActivationScopeError, CredentialActivationFacts};
use crate::disposition::Disposition;
use crate::record::Record;
use crate::record_id::RecordId;
use crate::record_scope::RecordScope;
use crate::stamper::Stamper;
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;

/// Records activation intents through an existing writer and its shared drain.
pub struct ActivationRecorder<S> {
    writer: AuditWriter<S>,
    scope: RecordScope,
    stamper: Stamper<SystemClock, UuidV7Generator>,
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
        Self {
            writer,
            scope,
            stamper: Stamper::new(SystemClock, UuidV7Generator),
        }
    }

    pub async fn record(&self, facts: CredentialActivationFacts) -> Result<RecordId, ActivationRecorderError> {
        let record = Record::credential_activation(self.scope.clone(), facts, self.stamper.stamp())?;
        let id = record.id();
        match self.writer.write(record).await? {
            Disposition::Stored => Ok(id),
            Disposition::Retrying | Disposition::Queued | Disposition::Dropped => {
                Err(ActivationRecorderError::NotStored)
            }
        }
    }
}

#[cfg(test)]
mod tests;
