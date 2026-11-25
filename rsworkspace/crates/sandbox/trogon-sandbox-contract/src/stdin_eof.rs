use serde::{Deserialize, Serialize};

/// Whether a stdin write also closes the command's stdin.
///
/// It serializes as the JSON boolean `eof`, `true` for `Close`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum StdinEof {
    #[default]
    Keep,
    Close,
}

impl StdinEof {
    pub const fn closes(self) -> bool {
        matches!(self, Self::Close)
    }
}

impl From<bool> for StdinEof {
    fn from(value: bool) -> Self {
        if value { Self::Close } else { Self::Keep }
    }
}

impl From<StdinEof> for bool {
    fn from(value: StdinEof) -> Self {
        value.closes()
    }
}
