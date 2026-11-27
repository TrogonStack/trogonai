use serde::Deserialize;

use crate::SnapshotState;

/// A snapshot as Daytona sends it, before the worker checks it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotWire {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) state: SnapshotState,
    #[serde(default)]
    pub(crate) organization_id: Option<String>,
    #[serde(default)]
    pub(crate) source_sandbox_id: Option<String>,
    #[serde(default)]
    pub(crate) general: bool,
    #[serde(default)]
    pub(crate) size: Option<f64>,
    #[serde(default)]
    pub(crate) error_reason: Option<String>,
}
