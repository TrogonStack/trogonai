use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WfpState {
    Absent,
    Installed,
    LoopbackMismatch,
    CannotRead,
}

impl WfpState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Installed => "installed",
            Self::LoopbackMismatch => "loopback-mismatch",
            Self::CannotRead => "cannot-read",
        }
    }
}
