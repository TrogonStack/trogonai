use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalError {
    Timeout,
    ChannelClosed,
    MalformedDecision,
}

impl fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("approval decision timed out"),
            Self::ChannelClosed => f.write_str("approval wait channel closed"),
            Self::MalformedDecision => f.write_str("malformed approval decision"),
        }
    }
}

impl std::error::Error for ApprovalError {}
