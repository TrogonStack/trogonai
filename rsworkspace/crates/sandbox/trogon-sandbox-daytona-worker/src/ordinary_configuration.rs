use std::collections::BTreeMap;

use serde::{Serialize, Serializer};
use trogon_sandbox_contract::StdinMode;

/// How the bridge starts the command.
#[derive(Debug, Serialize)]
pub(crate) struct OrdinaryConfiguration {
    pub(crate) argv: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) cwd: Option<String>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) user: Option<String>,
    #[serde(rename = "interactive", serialize_with = "interactive")]
    pub(crate) stdin: StdinMode,
}

/// The bridge takes the stdin mode as a JSON boolean.
fn interactive<S: Serializer>(mode: &StdinMode, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bool(*mode == StdinMode::Interactive)
}
