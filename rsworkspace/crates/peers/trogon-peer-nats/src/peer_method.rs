#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerMethod {
    Enroll,
    Heartbeat,
    Detach,
    Roster,
    Send,
    Status,
    PrepareBatch,
    AcknowledgeBatch,
}
