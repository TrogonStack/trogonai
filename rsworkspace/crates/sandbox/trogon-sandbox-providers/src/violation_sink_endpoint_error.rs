/// A NATS URL that cannot be turned into violation sink endpoints.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unusable NATS url for egress violation events: {0:?}")]
pub struct ViolationSinkEndpointError(pub String);
