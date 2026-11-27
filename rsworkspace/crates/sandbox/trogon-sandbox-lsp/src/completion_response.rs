use serde::{Deserialize, Serialize};

use crate::completion_item::CompletionItem;
use crate::completion_list::CompletionList;

/// The answer to `textDocument/completion`: a bare array or a list object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CompletionResponse {
    Items(Vec<CompletionItem>),
    List(CompletionList),
}

impl CompletionResponse {
    pub fn into_items(self) -> Vec<CompletionItem> {
        match self {
            Self::Items(items) => items,
            Self::List(list) => list.items,
        }
    }
}
