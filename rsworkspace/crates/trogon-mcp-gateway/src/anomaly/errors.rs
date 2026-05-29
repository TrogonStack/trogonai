use std::fmt;

#[derive(Debug)]
pub enum AnomalyError {
    TransportFailed(String),
    SerializeFailed(serde_json::Error),
}

impl fmt::Display for AnomalyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransportFailed(msg) => write!(f, "anomaly feature publish failed: {msg}"),
            Self::SerializeFailed(err) => write!(f, "anomaly feature serialization failed: {err}"),
        }
    }
}

impl std::error::Error for AnomalyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TransportFailed(_) => None,
            Self::SerializeFailed(err) => Some(err),
        }
    }
}
