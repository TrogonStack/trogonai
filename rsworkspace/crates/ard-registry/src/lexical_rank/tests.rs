use ard_catalog::CatalogEntry;
use ard_catalog::CatalogEntryWire;

use super::lexical_score;

fn sample_entry() -> CatalogEntry {
    CatalogEntryWire {
        identifier: "urn:air:example.com:agent:assistant".to_owned(),
        display_name: "Assistant".to_owned(),
        media_type: "application/a2a-agent-card+json".to_owned(),
        url: Some("https://example.com/card.json".to_owned()),
        data: None,
        description: Some("Helpful coding assistant".to_owned()),
        representative_queries: Some(vec!["write code".to_owned(), "debug rust".to_owned()]),
        tags: Some(vec!["coding".to_owned()]),
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
fn ranks_display_name_highest() {
    let entry = sample_entry();
    assert!(lexical_score("assistant", &entry) > lexical_score("chat", &entry));
}

#[test]
fn returns_zero_for_no_match() {
    let entry = sample_entry();
    assert_eq!(lexical_score("nonexistent-term", &entry), 0.0);
}
