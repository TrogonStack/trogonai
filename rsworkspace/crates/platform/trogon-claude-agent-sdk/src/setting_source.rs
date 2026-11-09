use serde::{Deserialize, Serialize};

/// Filesystem settings locations recognized by the pinned upstream SDKs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingSource {
    User,
    Project,
    Local,
}
