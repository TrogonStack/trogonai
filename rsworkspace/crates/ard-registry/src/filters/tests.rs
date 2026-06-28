use ard_catalog::{CatalogEntry, CatalogEntryWire, SearchFiltersWire};

use super::{entry_matches_filters, entry_matches_query};

fn sample_entry() -> CatalogEntry {
    CatalogEntryWire {
        identifier: "urn:air:example.com:agent:assistant".to_owned(),
        display_name: "Assistant".to_owned(),
        media_type: "application/a2a-agent-card+json".to_owned(),
        url: Some("https://example.com/card.json".to_owned()),
        data: None,
        description: Some("Helpful agent".to_owned()),
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
fn filters_by_media_type() {
    let entry = sample_entry();
    let filters = SearchFiltersWire {
        media_type: Some(vec!["application/mcp-server-card+json".to_owned()]),
        tags: None,
        capabilities: None,
    };
    assert!(!entry_matches_filters(&entry, Some(&filters)));
}

#[test]
fn filters_by_query() {
    let entry = sample_entry();
    assert!(entry_matches_query(&entry, Some("assistant")));
    assert!(!entry_matches_query(&entry, Some("missing")));
}
