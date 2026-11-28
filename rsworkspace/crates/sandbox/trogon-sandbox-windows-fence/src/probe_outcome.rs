use std::fmt;

/// Result of one connect attempt by `fence-helper wfp verify`. `Connected` and
/// `Refused` both mean the connect got past the filters; `Unreachable` and
/// `Error` say nothing about the fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    Blocked,
    Connected,
    Refused,
    Unreachable,
    Skipped,
    Error,
}

impl ProbeOutcome {
    /// Parse a reported outcome. A name this release does not know is refused:
    /// guessing could read an attempt that got through as one that did not.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "skipped" => Some(Self::Skipped),
            "blocked" => Some(Self::Blocked),
            "connected" => Some(Self::Connected),
            "refused" => Some(Self::Refused),
            "unreachable" => Some(Self::Unreachable),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    pub(crate) fn got_past_the_filters(&self) -> bool {
        matches!(self, Self::Connected | Self::Refused)
    }
}

impl fmt::Display for ProbeOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Blocked => "blocked",
            Self::Connected => "connected",
            Self::Refused => "refused",
            Self::Unreachable => "unreachable",
            Self::Skipped => "skipped",
            Self::Error => "error",
        })
    }
}
