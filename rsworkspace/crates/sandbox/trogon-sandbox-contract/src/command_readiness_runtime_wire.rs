use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

/// The JSON shape of a [`CommandReadinessRuntime`](crate::CommandReadinessRuntime), with the bare hex hash.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandReadinessRuntimeWire {
    pub(crate) sha256: String,
    pub(crate) protocol: NonZeroU32,
}
