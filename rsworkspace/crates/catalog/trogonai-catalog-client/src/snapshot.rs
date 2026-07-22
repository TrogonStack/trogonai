use buffa::{EnumValue, Message as _};

use crate::catalog_entry::{self, CatalogEntry};
use crate::error::CatalogClientError;
use trogonai_catalog_proto::{CatalogEntry as ProtoEntry, CatalogSnapshot as ProtoSnapshot};

#[derive(Debug, Clone, Default)]
pub struct CatalogSnapshot {
    pub entries: Vec<CatalogEntry>,
}

impl CatalogSnapshot {
    pub fn from_proto(proto: &ProtoSnapshot) -> Result<Self, CatalogClientError> {
        let entries = proto
            .entries
            .iter()
            .map(|e| CatalogEntry {
                model_id: e.model_id.clone(),
                provider: e.provider.clone(),
                context_window: e.context_window,
                max_output: e.max_output,
                modality: catalog_entry::modality_from_proto(e.modality),
            })
            .collect();
        Ok(Self { entries })
    }

    pub fn to_proto(&self) -> ProtoSnapshot {
        ProtoSnapshot {
            entries: self
                .entries
                .iter()
                .map(|e| ProtoEntry {
                    model_id: e.model_id.clone(),
                    provider: e.provider.clone(),
                    context_window: e.context_window,
                    max_output: e.max_output,
                    modality: EnumValue::Known(e.modality),
                    __buffa_unknown_fields: Default::default(),
                })
                .collect(),
            fetched_at_unix_secs: 0,
            __buffa_unknown_fields: Default::default(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, CatalogClientError> {
        Ok(self.to_proto().encode_to_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CatalogClientError> {
        let proto = ProtoSnapshot::decode_from_slice(bytes).map_err(|e| CatalogClientError::Decode(e.to_string()))?;
        Self::from_proto(&proto)
    }
}
