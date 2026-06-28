use ard_catalog::{ArdIdentifier, ArdStorageKey, CatalogEntry, CatalogEntryWire};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "eventType", rename_all = "camelCase")]
pub enum CatalogEvent {
    Upserted {
        storage_key: String,
        entry: Box<CatalogEntryWire>,
    },
    Deleted {
        identifier: String,
        storage_key: String,
    },
    Validated {
        identifier: String,
        storage_key: String,
    },
    Indexed {
        identifier: String,
        storage_key: String,
    },
}

impl CatalogEvent {
    pub fn upserted(entry: &CatalogEntry) -> Self {
        Self::Upserted {
            storage_key: ArdStorageKey::from_identifier(entry.identifier()).to_string(),
            entry: Box::new(entry.clone().into_wire()),
        }
    }

    pub fn deleted(identifier: &ArdIdentifier) -> Self {
        Self::Deleted {
            identifier: identifier.to_string(),
            storage_key: ArdStorageKey::from_identifier(identifier).to_string(),
        }
    }

    pub fn storage_key(&self) -> &str {
        match self {
            Self::Upserted { storage_key, .. }
            | Self::Deleted { storage_key, .. }
            | Self::Validated { storage_key, .. }
            | Self::Indexed { storage_key, .. } => storage_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use ard_catalog::CatalogEntryWire;

    use super::CatalogEvent;

    fn entry() -> ard_catalog::CatalogEntry {
        CatalogEntryWire {
            identifier: "urn:air:example.com:agent:assistant".to_owned(),
            display_name: "Assistant".to_owned(),
            media_type: "application/a2a-agent-card+json".to_owned(),
            url: Some("https://example.com/card.json".to_owned()),
            data: None,
            description: Some("Helpful assistant".to_owned()),
            representative_queries: Some(vec!["help me".to_owned(), "answer questions".to_owned()]),
            tags: Some(vec!["demo".to_owned()]),
            capabilities: Some(vec!["chat".to_owned()]),
            version: None,
            updated_at: None,
            metadata: None,
            trust_manifest: None,
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn event_round_trips_as_json() {
        let event = CatalogEvent::upserted(&entry());
        let json = serde_json::to_vec(&event).unwrap();
        let decoded: CatalogEvent = serde_json::from_slice(&json).unwrap();
        assert_eq!(decoded, event);
    }
}
