use serde::{Deserialize, Serialize};

/// The position of a stdin write within one execution.
///
/// It starts at 0 and is gapless, and it serializes as a JSON number.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, Default)]
pub struct StdinSeq(u64);

impl StdinSeq {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}
