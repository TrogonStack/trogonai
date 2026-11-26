use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{Digest, OciImageRef};

/// The JSON shape of a [`ResolvedContainerImage`](crate::ResolvedContainerImage) before its
/// parts are checked against each other.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ResolvedContainerImageWire {
    pub(crate) reference: OciImageRef,
    pub(crate) digest: Digest,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) working_dir: Option<String>,
    #[serde(default)]
    pub(crate) user: Option<String>,
    #[serde(default)]
    pub(crate) compressed_bytes: u64,
}
