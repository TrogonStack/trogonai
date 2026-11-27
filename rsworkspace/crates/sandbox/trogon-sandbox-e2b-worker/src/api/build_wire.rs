use serde::Deserialize;

use crate::snapshots::SnapshotBuildId;

/// One build of a template.
#[derive(Deserialize)]
pub(crate) struct BuildWire {
    #[serde(rename = "buildID")]
    pub(crate) build_id: SnapshotBuildId,
    pub(crate) status: String,
}
