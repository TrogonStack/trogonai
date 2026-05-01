#[derive(Debug)]
pub enum ApprovalError {
    Nats(String),
    JetStream(String),
    Serialize(String),
    Vault(String),
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nats(s)      => write!(f, "NATS error: {s}"),
            Self::JetStream(s) => write!(f, "JetStream error: {s}"),
            Self::Serialize(s) => write!(f, "Serialization error: {s}"),
            Self::Vault(s)     => write!(f, "Vault error: {s}"),
        }
    }
}

impl std::error::Error for ApprovalError {}
