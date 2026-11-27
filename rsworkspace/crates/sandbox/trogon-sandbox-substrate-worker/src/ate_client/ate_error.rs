use tonic::Code;

/// What a failed `ateapi` call means to us.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum AteError {
    /// The resource is already there. Every create we issue is idempotent, so this is a
    /// signal to fall through to a get, not a failure.
    #[error("already exists: {message}")]
    AlreadyExists { message: String },
    /// The resource is gone. Cancel treats this as success; exec treats it as a lost lease.
    #[error("not found: {message}")]
    NotFound { message: String },
    /// The control plane is not ready for this yet: a template version still building, a
    /// delete of an actor that has not finished suspending. Retry inside the phase deadline.
    #[error("failed precondition: {message}")]
    FailedPrecondition { message: String },
    /// No eligible worker has room. Retryable, and on deadline the lease should fail in a
    /// way that lets the broker try another provider.
    #[error("resource exhausted: {message}")]
    ResourceExhausted { message: String },
    /// Our identity is wrong. A deployment fault: retrying cannot fix it and hiding it as a
    /// tenant-visible lease failure would send the wrong person looking.
    #[error("permission denied: {message}")]
    PermissionDenied { message: String },
    /// The control plane is flapping. Bounded retry with backoff.
    #[error("control plane unavailable: {message}")]
    Unavailable { message: String },
    /// We built a bad request. A bug in this crate; tests assert it never happens.
    #[error("invalid argument: {message}")]
    InvalidArgument { message: String },
    /// Anything else, including transport errors.
    #[error("substrate error: {message}")]
    Other { message: String },
}

impl AteError {
    /// Whether retrying the same call unchanged could plausibly succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::FailedPrecondition { .. } | Self::ResourceExhausted { .. } | Self::Unavailable { .. }
        )
    }

    pub fn message(&self) -> &str {
        match self {
            Self::AlreadyExists { message }
            | Self::NotFound { message }
            | Self::FailedPrecondition { message }
            | Self::ResourceExhausted { message }
            | Self::PermissionDenied { message }
            | Self::Unavailable { message }
            | Self::InvalidArgument { message }
            | Self::Other { message } => message,
        }
    }
}

impl From<tonic::Status> for AteError {
    fn from(status: tonic::Status) -> Self {
        let message = status.message().to_string();
        match status.code() {
            Code::AlreadyExists => Self::AlreadyExists { message },
            Code::NotFound => Self::NotFound { message },
            Code::FailedPrecondition => Self::FailedPrecondition { message },
            Code::ResourceExhausted => Self::ResourceExhausted { message },
            Code::PermissionDenied | Code::Unauthenticated => Self::PermissionDenied { message },
            Code::Unavailable | Code::DeadlineExceeded | Code::Aborted => Self::Unavailable { message },
            Code::InvalidArgument => Self::InvalidArgument { message },
            other => Self::Other {
                message: format!("{other:?}: {message}"),
            },
        }
    }
}
