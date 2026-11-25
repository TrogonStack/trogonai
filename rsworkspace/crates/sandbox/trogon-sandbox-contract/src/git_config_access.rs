use serde::{Deserialize, Serialize};

/// Whether `.git/config` stays writable where the rest of the git directory is protected. Hooks stay protected either way.
///
/// It serializes as a JSON boolean, `true` for `Writable`, so existing peers keep parsing it. An omitted value reads
/// as `Protected`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum GitConfigAccess {
    #[default]
    Protected,
    Writable,
}

impl GitConfigAccess {
    pub const fn is_writable(self) -> bool {
        matches!(self, Self::Writable)
    }
}

impl From<bool> for GitConfigAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::Writable } else { Self::Protected }
    }
}

impl From<GitConfigAccess> for bool {
    fn from(value: GitConfigAccess) -> Self {
        value.is_writable()
    }
}
