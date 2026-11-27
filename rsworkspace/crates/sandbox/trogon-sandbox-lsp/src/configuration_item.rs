use serde::{Deserialize, Serialize};

/// One entry of a `workspace/configuration` request.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}
