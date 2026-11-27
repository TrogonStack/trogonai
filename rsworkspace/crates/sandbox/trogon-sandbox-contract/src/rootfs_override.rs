use serde::{Deserialize, Serialize};

/// Whether a lease asks to leave the root filesystem writable on a worker that makes it read only.
///
/// It serializes as a JSON boolean, `true` for `Writable`, so existing peers keep parsing it. An omitted value reads
/// as `Unchanged`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum RootfsOverride {
    #[default]
    Unchanged,
    Writable,
}

impl RootfsOverride {
    pub const fn is_writable(self) -> bool {
        matches!(self, Self::Writable)
    }
}

impl From<bool> for RootfsOverride {
    fn from(flag: bool) -> Self {
        if flag { Self::Writable } else { Self::Unchanged }
    }
}

impl From<RootfsOverride> for bool {
    fn from(value: RootfsOverride) -> Self {
        value.is_writable()
    }
}
