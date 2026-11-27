use serde::{Deserialize, Serialize};

use crate::MountAccess;

/// The JSON shape of a [`WasiPreopenSpec`](crate::WasiPreopenSpec) before its rules are checked.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct WasiPreopenSpecWire {
    pub(crate) host_dir: String,
    pub(crate) guest_path: String,
    #[serde(default, rename = "readonly")]
    pub(crate) access: MountAccess,
}
