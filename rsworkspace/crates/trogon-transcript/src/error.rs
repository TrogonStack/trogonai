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

#[cfg(test)]
mod tests {
    use super::*;

    fn json_err() -> serde_json::Error {
        serde_json::from_str::<u32>("{bad}").unwrap_err()
    }

    #[test]
    fn display_publish() {
        assert!(format!("{}", TranscriptError::Publish("x".into())).contains("publish error"));
    }

    #[test]
    fn display_provision() {
        assert!(format!("{}", TranscriptError::Provision("x".into())).contains("provision error"));
    }

    #[test]
    fn display_serialization() {
        assert!(format!("{}", TranscriptError::Serialization(json_err()))
            .contains("serialization error"));
    }

    #[test]
    fn display_deserialization() {
        assert!(format!("{}", TranscriptError::Deserialization(json_err()))
            .contains("deserialization error"));
    }

    #[test]
    fn display_stream() {
        assert!(format!("{}", TranscriptError::Stream("x".into())).contains("stream error"));
    }
}
