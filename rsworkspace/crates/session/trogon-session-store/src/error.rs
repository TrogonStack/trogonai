#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session key is invalid")]
    InvalidKey,
    #[error("Session history is unsupported or invalid")]
    UnsupportedHistory,
    #[error("Session input exceeds the bounded lifecycle profile")]
    Limit,
    #[error("Session creation conflicts with original evidence")]
    CreationConflict,
    #[error("Session source changed before the guarded transition")]
    SourceChanged,
    #[error("Session append acknowledgement remains uncertain")]
    AppendUnknown,
    #[error("Session admission was refused")]
    Admission(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Session persistence is unavailable")]
    Persistence(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Session fact authentication failed")]
    Authentication(#[source] Box<dyn std::error::Error + Send + Sync>),
}
