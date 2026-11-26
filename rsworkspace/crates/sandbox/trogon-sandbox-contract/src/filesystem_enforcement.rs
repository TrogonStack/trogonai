use serde::{Deserialize, Serialize};

/// Whether filesystem rules apply at all. `Disabled` applies none: everything is readable and writable, and the mandatory protections do not apply.
///
/// It serializes as a JSON boolean, `true` for `Disabled`, so existing peers keep parsing it. An omitted value reads
/// as `Enforced`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FilesystemEnforcement {
    #[default]
    Enforced,
    Disabled,
}

impl FilesystemEnforcement {
    pub const fn is_disabled(self) -> bool {
        matches!(self, Self::Disabled)
    }
}

impl From<bool> for FilesystemEnforcement {
    fn from(flag: bool) -> Self {
        if flag { Self::Disabled } else { Self::Enforced }
    }
}

impl From<FilesystemEnforcement> for bool {
    fn from(value: FilesystemEnforcement) -> Self {
        value.is_disabled()
    }
}
