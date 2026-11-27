use serde::{Deserialize, Serialize};

use crate::Port;

/// The JSON shape of a [`WasiEgressRule`](crate::WasiEgressRule) before its rules are checked.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct WasiEgressRuleWire {
    pub(crate) host: String,
    pub(crate) port: Port,
}
