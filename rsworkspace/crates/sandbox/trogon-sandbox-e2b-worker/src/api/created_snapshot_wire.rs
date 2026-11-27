use serde::Deserialize;

/// Reply of the snapshot capture call.
#[derive(Deserialize)]
pub(crate) struct CreatedSnapshotWire {
    #[serde(rename = "snapshotID")]
    pub(crate) id: String,
    pub(crate) names: Vec<String>,
}
