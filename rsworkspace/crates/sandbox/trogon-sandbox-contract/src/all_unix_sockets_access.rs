use serde::{Deserialize, Serialize};

/// Whether the payload may use every Unix socket, wherever its path. On Linux it covers `AF_UNIX` and `AF_VSOCK` sockets, which reaches every socket file visible in the sandbox.
///
/// It serializes as a JSON boolean, `true` for `Allowed`, so existing peers keep parsing it. An omitted value reads
/// as `Denied`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum AllUnixSocketsAccess {
    #[default]
    Denied,
    Allowed,
}

impl AllUnixSocketsAccess {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

impl From<bool> for AllUnixSocketsAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::Allowed } else { Self::Denied }
    }
}

impl From<AllUnixSocketsAccess> for bool {
    fn from(value: AllUnixSocketsAccess) -> Self {
        value.is_allowed()
    }
}
