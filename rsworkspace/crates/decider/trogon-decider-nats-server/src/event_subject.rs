//! Where a module's events live.
//!
//! One subtree per module, `{module_name}.events.{stream_id}`, so a module's
//! history is addressable on its own and a stream's events never share a
//! subject with another module's. The scheduler already writes its schedule
//! events under exactly this shape, so the host inherits a topology that is in
//! production rather than inventing a second one beside it.

use std::sync::Arc;

use async_nats::{SubjectError, jetstream};
use trogon_decider_nats::{
    StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_decider_wasm_runtime::ModuleName;
use trogon_nats::{SubjectViolationError, validate_published_subject};

use crate::constants::EVENT_SUBJECT_SEGMENT;

/// Resolves one module's stream ids onto its event subjects.
#[derive(Debug, Clone)]
pub struct ModuleEventSubjects {
    prefix: Arc<str>,
}

impl ModuleEventSubjects {
    /// Binds the resolver to the module whose events it addresses.
    pub fn new(module_name: &ModuleName) -> Self {
        Self {
            prefix: Arc::from(format!("{}.{EVENT_SUBJECT_SEGMENT}.", module_name.as_str())),
        }
    }

    /// The subject carrying one stream's events.
    ///
    /// Validated as a *published* subject, not merely as a subject: the stream
    /// id comes from the guest, and a guest that returns `>` would otherwise
    /// resolve onto the module's whole subtree.
    pub fn subject_for(&self, stream_id: &str) -> Result<StreamSubject, EventSubjectError> {
        let subject = format!("{}{stream_id}", self.prefix);
        validate_published_subject(&subject).map_err(|source| EventSubjectError::NotPublishable {
            stream_id: stream_id.to_owned(),
            source,
        })?;

        StreamSubject::new(subject).map_err(|source| EventSubjectError::Subject {
            stream_id: stream_id.to_owned(),
            source,
        })
    }

    /// The pattern the events stream must capture for this module.
    pub fn subscription_pattern(&self) -> String {
        format!("{}>", self.prefix)
    }
}

impl StreamSubjectResolver<str> for ModuleEventSubjects {
    type Error = EventSubjectError;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let subject = self.subject_for(stream_id)?;
        let current_position = subject_current_position(events_stream, &subject).await?;

        Ok(SubjectState {
            subject,
            current_position,
        })
    }
}

/// Why a stream id could not be resolved to a subject and its position.
#[derive(Debug, thiserror::Error)]
pub enum EventSubjectError {
    /// The stream id resolves onto a subject that cannot be published to.
    #[error("stream id '{stream_id}' does not form a publishable event subject: {source}")]
    NotPublishable {
        stream_id: String,
        #[source]
        source: SubjectViolationError,
    },
    /// The stream id does not form a subject NATS will carry.
    #[error("stream id '{stream_id}' does not form a valid event subject: {source}")]
    Subject {
        stream_id: String,
        #[source]
        source: SubjectError,
    },
    /// Reading the subject's current position failed.
    #[error("failed to read the current position of a module event subject: {0}")]
    CurrentPosition(#[from] StreamStoreError),
}

#[cfg(test)]
mod tests;
