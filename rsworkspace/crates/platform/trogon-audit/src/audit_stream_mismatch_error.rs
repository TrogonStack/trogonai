/// An existing audit stream that would weaken a guarantee the writer relies on.
///
/// Each variant names the rule and the stream, never a stored value; the module
/// docs of [`AuditStream`](crate::AuditStream) say why each rule exists.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuditStreamMismatchError {
    #[error("stream '{stream}' must use file storage")]
    Storage { stream: &'static str },
    #[error("stream '{stream}' must use limits retention")]
    Retention { stream: &'static str },
    #[error("stream '{stream}' must refuse writes when full, not discard the oldest record")]
    Discard { stream: &'static str },
    #[error("stream '{stream}' must deny deleting a record")]
    DenyDelete { stream: &'static str },
    #[error("stream '{stream}' must deny purging records")]
    DenyPurge { stream: &'static str },
    #[error("stream '{stream}' must not expire records by age")]
    MaxAge { stream: &'static str },
    #[error("stream '{stream}' must not let a record set its own expiry")]
    MessageTtl { stream: &'static str },
    #[error("stream '{stream}' must not limit the records kept per subject")]
    MaxMessagesPerSubject { stream: &'static str },
    #[error("stream '{stream}' must keep a duplicate window of two minutes or longer")]
    DuplicateWindow { stream: &'static str },
    #[error("stream '{stream}' must keep at least the replicas the deployment asked for")]
    Replicas { stream: &'static str },
    #[error("stream '{stream}' is missing a subject the audit kinds are published on: '{subject}'")]
    MissingSubject { stream: &'static str, subject: String },
}

#[cfg(test)]
mod tests;
