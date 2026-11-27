use serde::{Deserialize, Serialize};

/// Which administrative destination class can receive session data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyMode {
    LocalOnly,
    ApprovedDestinations,
}
