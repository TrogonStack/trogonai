use serde::Deserialize;

use super::build_wire::BuildWire;
use crate::snapshots::{SnapshotBuildId, SnapshotTemplateId};

/// Reply of `GET /templates/{id}`.
#[derive(Deserialize)]
pub(crate) struct TemplateWire {
    #[serde(rename = "templateID")]
    pub(crate) template_id: SnapshotTemplateId,
    #[serde(rename = "buildID")]
    pub(crate) build_id: SnapshotBuildId,
    #[serde(rename = "buildCount")]
    pub(crate) build_count: u32,
    pub(crate) builds: Vec<BuildWire>,
}
