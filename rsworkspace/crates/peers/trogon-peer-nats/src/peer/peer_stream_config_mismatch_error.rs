#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerStreamConfigMismatchError {
    #[error("peer event stream must reject new writes when full")]
    Discard,
    #[error("peer event stream must retain accepted events")]
    Retention,
    #[error("peer event stream byte budget differs from its declared bound")]
    MaxBytes,
    #[error("peer event stream must not expire accepted events")]
    MaxAge,
    #[error("peer event stream must not evict by event count")]
    MaxMessages,
    #[error("peer event stream must not evict by per-peer event count")]
    MaxMessagesPerSubject,
    #[error("peer event stream must support atomic event batches")]
    AtomicPublish,
    #[error("peer event stream subject filter differs from its tenant scope")]
    Subjects,
    #[error("peer event stream must use file storage")]
    Storage,
    #[error("peer event stream must not replace retained history through rollups")]
    Rollup,
    #[error("peer event stream must not admit per-message expiry")]
    MessageTtl,
    #[error("peer event stream must not import another stream's history")]
    Sources,
}
