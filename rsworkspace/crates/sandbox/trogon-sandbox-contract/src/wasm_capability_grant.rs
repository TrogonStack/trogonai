use serde::{Deserialize, Serialize};

use crate::{WasiEgressRule, WasiPreopenSpec};

/// An opt-in, narrowly scoped capability grant for a WASM guest.
///
/// A spec with no grant runs the guest fully deny-by-default: no preopened directories and no outbound sockets. A
/// grant only adds what `preopens` and `allowed_egress` list, and it has no way to relax that default further. Each
/// empty list is omitted from the wire.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct WasmCapabilityGrant {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preopens: Vec<WasiPreopenSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_egress: Vec<WasiEgressRule>,
}
