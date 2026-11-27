use serde::{Deserialize, Serialize};

use crate::location::Location;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInformation {
    pub name: String,
    pub kind: u32,
    pub location: Location,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
}
