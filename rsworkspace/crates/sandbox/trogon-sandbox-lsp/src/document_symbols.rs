use serde::{Deserialize, Serialize};

use crate::document_symbol::DocumentSymbol;
use crate::symbol_information::SymbolInformation;

/// The answer to `textDocument/documentSymbol`: a tree of symbols or a flat list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DocumentSymbols {
    Hierarchical(Vec<DocumentSymbol>),
    Flat(Vec<SymbolInformation>),
}

impl DocumentSymbols {
    /// Top-level symbol names.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::Hierarchical(symbols) => symbols.iter().map(|s| s.name.as_str()).collect(),
            Self::Flat(symbols) => symbols.iter().map(|s| s.name.as_str()).collect(),
        }
    }
}
