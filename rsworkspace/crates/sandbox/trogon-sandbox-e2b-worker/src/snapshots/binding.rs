use serde::{Deserialize, Serialize};

use super::build_id::SnapshotBuildId;
use super::template_id::SnapshotTemplateId;

/// The immutable template and build a snapshot restores from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotBinding {
    pub(crate) template_id: SnapshotTemplateId,
    pub(crate) build_id: SnapshotBuildId,
}

impl SnapshotBinding {
    /// The `template:build` selector E2B accepts to restore this exact build.
    pub(crate) fn restore_selector(&self) -> String {
        format!("{}:{}", self.template_id.as_str(), String::from(self.build_id))
    }
}
