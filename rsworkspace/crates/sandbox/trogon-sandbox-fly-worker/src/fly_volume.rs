use serde::Deserialize;

use crate::FlyVolumeId;

/// A Fly volume as the Machines API reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct FlyVolume {
    pub id: FlyVolumeId,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub attached_machine_id: Option<String>,
}
