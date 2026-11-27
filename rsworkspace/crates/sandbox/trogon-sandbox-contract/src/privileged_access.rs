use serde::{Deserialize, Serialize};

/// Whether a lease asks for elevated capabilities, such as `NET_ADMIN`, `SYS_ADMIN`, or `/dev/fuse`, to run VPN
/// clients, FUSE filesystems, or nested container runtimes.
///
/// It serializes as a JSON boolean, `true` for `Granted`, so existing peers keep parsing it. An omitted value reads
/// as `Denied`. Only a strong-isolation provider may grant it, and a weaker one must refuse the lease rather than
/// ignore the request; the contract only carries the request.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum PrivilegedAccess {
    #[default]
    Denied,
    Granted,
}

impl PrivilegedAccess {
    pub const fn is_granted(self) -> bool {
        matches!(self, Self::Granted)
    }
}

impl From<bool> for PrivilegedAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::Granted } else { Self::Denied }
    }
}

impl From<PrivilegedAccess> for bool {
    fn from(value: PrivilegedAccess) -> Self {
        value.is_granted()
    }
}
