use crate::catalog_entry_wire::CatalogEntryWire;
use crate::catalog_manifest_wire::{CatalogManifestWire, SPEC_VERSION};

#[test]
fn exposes_entries_immutably() {
    let manifest: crate::catalog_manifest::CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![CatalogEntryWire {
            identifier: "urn:air:example.com:agent:assistant".to_owned(),
            display_name: "Assistant".to_owned(),
            media_type: "application/a2a-agent-card+json".to_owned(),
            url: Some("https://example.com/card.json".to_owned()),
            data: None,
            description: None,
            representative_queries: None,
            tags: None,
            capabilities: None,
            version: None,
            updated_at: None,
            metadata: None,
            trust_manifest: None,
        }],
    }
    .try_into()
    .unwrap();
    assert_eq!(manifest.entries().len(), 1);
    assert_eq!(manifest.spec_version(), SPEC_VERSION);
}
