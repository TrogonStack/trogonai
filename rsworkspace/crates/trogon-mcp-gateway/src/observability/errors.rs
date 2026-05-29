use std::fmt;

/// Errors from observability bootstrap (OTel export, audit→SIEM bridge).
#[derive(Debug)]
pub enum ObservabilityError {
    Config(String),
    OtelInit(String),
    AuditBridge(String),
    Reshape(String),
}

impl fmt::Display for ObservabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(message) => write!(f, "observability config: {message}"),
            Self::OtelInit(message) => write!(f, "otel exporter init: {message}"),
            Self::AuditBridge(message) => write!(f, "audit bridge: {message}"),
            Self::Reshape(message) => write!(f, "audit reshape: {message}"),
        }
    }
}

impl std::error::Error for ObservabilityError {}
