use serde::Deserialize;

/// A volume snapshot as the Machines API lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct FlySnapshot {
    pub id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub retention_days: Option<u64>,
    #[serde(default)]
    pub size: Option<u64>,
}
