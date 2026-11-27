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
    Other(String),
}

impl ProbeOutcome {
    pub(crate) fn parse(s: Option<&str>) -> Self {
        match s {
            None | Some("skipped") => Self::Skipped,
            Some("blocked") => Self::Blocked,
            Some("connected") => Self::Connected,
            Some("refused") => Self::Refused,
            Some("unreachable") => Self::Unreachable,
            Some("error") => Self::Error,
            Some(other) => Self::Other(other.to_string()),
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
            Self::Other(s) => s,
        })
    }
}
