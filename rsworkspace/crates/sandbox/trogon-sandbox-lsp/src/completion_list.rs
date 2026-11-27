use serde::{Deserialize, Serialize};

use crate::completion_item::CompletionItem;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionList {
    pub is_incomplete: bool,
    pub items: Vec<CompletionItem>,
}
