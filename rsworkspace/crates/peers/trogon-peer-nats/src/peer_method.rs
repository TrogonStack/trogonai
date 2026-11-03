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

impl PeerMethod {
    pub(crate) const ALL: [Self; 8] = [
        Self::Enroll,
        Self::Heartbeat,
        Self::Detach,
        Self::Roster,
        Self::Send,
        Self::Status,
        Self::PrepareBatch,
        Self::AcknowledgeBatch,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enroll => "Enroll",
            Self::Heartbeat => "Heartbeat",
            Self::Detach => "Detach",
            Self::Roster => "Roster",
            Self::Send => "Send",
            Self::Status => "Status",
            Self::PrepareBatch => "PrepareBatch",
            Self::AcknowledgeBatch => "AcknowledgeBatch",
        }
    }

    pub(crate) fn from_terminal(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.as_str() == value)
    }
}
