use std::fmt;

#[derive(Debug)]
pub enum TranscriptError {
    /// Failed to publish a transcript entry to JetStream.
    Publish(String),
    /// Failed to provision (create) the JetStream stream.
    Provision(String),
    /// Failed to serialize an entry to JSON before publishing.
    Serialization(serde_json::Error),
    /// Failed to deserialize a raw JetStream message into a `TranscriptEntry`.
    Deserialization(serde_json::Error),
    /// Error reading from or creating a JetStream consumer.
    Stream(String),
}

impl fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranscriptError::Publish(msg) => write!(f, "transcript publish error: {msg}"),
            TranscriptError::Provision(msg) => write!(f, "transcript provision error: {msg}"),
            TranscriptError::Serialization(e) => {
                write!(f, "transcript serialization error: {e}")
            }
            TranscriptError::Deserialization(e) => {
                write!(f, "transcript deserialization error: {e}")
            }
            TranscriptError::Stream(msg) => write!(f, "transcript stream error: {msg}"),
        }
    }
}

impl std::error::Error for TranscriptError {}
