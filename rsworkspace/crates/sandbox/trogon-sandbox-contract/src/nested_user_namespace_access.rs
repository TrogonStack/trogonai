use serde::{Deserialize, Serialize};

/// Whether the payload may create user and mount namespaces and mount inside them on Linux.
///
/// It serializes as a JSON boolean, `true` for `Allowed`, so existing peers keep parsing it. An omitted value reads
/// as `Denied`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum NestedUserNamespaceAccess {
    #[default]
    Denied,
    Allowed,
}

impl NestedUserNamespaceAccess {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

impl From<bool> for NestedUserNamespaceAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::Allowed } else { Self::Denied }
    }
}

impl From<NestedUserNamespaceAccess> for bool {
    fn from(value: NestedUserNamespaceAccess) -> Self {
        value.is_allowed()
    }
}
