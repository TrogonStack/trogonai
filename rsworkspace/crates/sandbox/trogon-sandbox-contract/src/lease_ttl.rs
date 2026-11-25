use serde::{Deserialize, Serialize};

/// Whether a lease has a wall-clock deadline.
///
/// `Unbounded` asks for a lease with no deadline, which runs until it is released or goes idle, so the spec that
/// carries it must set an idle release. It serializes as the JSON boolean `no_ttl`, `true` for `Unbounded`, so
/// existing peers keep parsing it. An omitted value reads as `Bounded`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LeaseTtl {
    #[default]
    Bounded,
    Unbounded,
}

impl LeaseTtl {
    pub const fn is_unbounded(self) -> bool {
        matches!(self, Self::Unbounded)
    }

    /// Reports whether this is the default, so the wire can leave the field out.
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Bounded)
    }
}

impl From<bool> for LeaseTtl {
    fn from(flag: bool) -> Self {
        if flag { Self::Unbounded } else { Self::Bounded }
    }
}

impl From<LeaseTtl> for bool {
    fn from(value: LeaseTtl) -> Self {
        value.is_unbounded()
    }
}
