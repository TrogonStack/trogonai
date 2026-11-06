#[derive(Debug, thiserror::Error)]
pub enum LivePolicyError {
    #[error("policy bucket is unavailable")]
    Bucket(#[source] async_nats::jetstream::context::KeyValueError),
    #[error("policy stream information is unavailable")]
    Information(#[source] async_nats::jetstream::stream::InfoError),
    #[error("policy entry is unavailable")]
    Entry(#[source] async_nats::jetstream::stream::LastRawMessageError),
    #[error("policy operation timed out")]
    Timeout,
    #[error("policy connection is unavailable")]
    Disconnected,
    #[error("policy bucket configuration is incompatible")]
    BucketConfiguration,
    #[error("policy stream incarnation changed")]
    StreamIncarnation,
    #[error("policy entry is not a put for the exact scoped key")]
    Removed,
    #[error("policy envelope is invalid")]
    Envelope,
    #[error("policy envelope exceeds its resource bounds")]
    Bounds,
    #[error("policy envelope names another scope")]
    Scope,
    #[error("policy envelope names another authority incarnation")]
    Incarnation,
    #[error("policy authority regressed or equivocated")]
    Regression,
    #[error("policy envelope is not active")]
    Inactive,
    #[error("policy lease exceeds the hard freshness bound")]
    Lease,
    #[error("policy content is invalid")]
    Policy,
    #[error("policy state is unavailable")]
    State,
}

impl LivePolicyError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Bucket(_) | Self::Information(_) | Self::Entry(_) => "transport",
            Self::Timeout => "timeout",
            Self::Disconnected => "disconnected",
            Self::BucketConfiguration => "bucket_configuration",
            Self::StreamIncarnation => "stream_incarnation",
            Self::Removed => "removed",
            Self::Envelope | Self::Policy => "invalid_document",
            Self::Bounds => "resource_bounds",
            Self::Scope => "scope",
            Self::Incarnation => "authority_incarnation",
            Self::Regression => "regression",
            Self::Inactive => "inactive",
            Self::Lease => "lease",
            Self::State => "state",
        }
    }
}
