/// The JetStream stream that captures a subject's messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpStream {
    Commands,
    Responses,
    ClientOps,
    Notifications,
    Global,
    GlobalExt,
}

impl AcpStream {
    pub fn suffix(&self) -> &'static str {
        match self {
            Self::Commands => "COMMANDS",
            Self::Responses => "RESPONSES",
            Self::ClientOps => "CLIENT_OPS",
            Self::Notifications => "NOTIFICATIONS",
            Self::Global => "GLOBAL",
            Self::GlobalExt => "GLOBAL_EXT",
        }
    }
}

impl std::fmt::Display for AcpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.suffix())
    }
}

/// A subject knows which stream captures it (if any).
pub trait StreamAssignment {
    const STREAM: Option<AcpStream>;
}
