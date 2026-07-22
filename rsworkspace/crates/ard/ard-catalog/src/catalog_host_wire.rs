//! Untrusted ARD catalog host wire shape.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalog_host::{CatalogHost, CatalogHostError};

/// Untrusted ARD catalog host wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogHostWire(pub Value);

impl CatalogHostWire {
    pub fn into_domain(self) -> Result<CatalogHost, CatalogHostError> {
        CatalogHost::new(self.0)
    }
}

impl TryFrom<CatalogHostWire> for CatalogHost {
    type Error = CatalogHostError;

    fn try_from(wire: CatalogHostWire) -> Result<Self, Self::Error> {
        CatalogHost::new(wire.0)
    }
}

#[cfg(test)]
mod tests;
