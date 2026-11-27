use serde::{Deserialize, Serialize};

use crate::{
    AllUnixSocketsAccess, AppleEventsAccess, LiteralPath, LocalBindingAccess, MachServiceName, ProtectedNamesAnchor,
    PtyAccess, TrustServiceAccess,
};

/// What a lease opts into on macOS, each off by default.
///
/// Each one widens the profile beyond the stricter default.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MacosOptions {
    /// Pseudo-terminal devices.
    #[serde(default)]
    pub allow_pty: PtyAccess,
    /// Binding and accepting on any local port and connecting to loopback on any port.
    #[serde(default)]
    pub allow_local_binding: LocalBindingAccess,
    /// Extra Mach services the payload may look up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_mach_lookup: Vec<MachServiceName>,
    /// Lookup of the trust agent service.
    #[serde(default)]
    pub allow_trust_service: TrustServiceAccess,
    /// Unix sockets under these paths may be created, bound and connected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_unix_sockets: Vec<LiteralPath>,
    /// Every unix socket, wherever its path.
    #[serde(default)]
    pub allow_all_unix_sockets: AllUnixSocketsAccess,
    /// Sending Apple events and the launch services they need.
    #[serde(default)]
    pub allow_apple_events: AppleEventsAccess,
    /// Where the mandatory protections are laid out from.
    #[serde(default, skip_serializing_if = "ProtectedNamesAnchor::is_default")]
    pub protected_names_anchor: ProtectedNamesAnchor,
}

impl MacosOptions {
    /// Reports whether nothing is opted into, so the wire can leave the field out.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
