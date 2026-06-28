use ard_catalog::SearchFiltersWire;

use super::SearchFilters;

#[test]
fn from_wire_none_is_empty() {
    let filters = SearchFilters::from_wire(None);
    assert!(filters.is_empty());
    assert!(filters.media_types().is_empty());
    assert!(filters.tags().is_empty());
    assert!(filters.capabilities().is_empty());
}

#[test]
fn from_wire_maps_each_field() {
    let wire = SearchFiltersWire {
        media_type: Some(vec!["application/a2a-agent-card+json".to_owned()]),
        tags: Some(vec!["demo".to_owned()]),
        capabilities: Some(vec!["chat".to_owned()]),
    };
    let filters = SearchFilters::from_wire(Some(wire));
    assert!(!filters.is_empty());
    assert_eq!(filters.media_types(), &["application/a2a-agent-card+json"]);
    assert_eq!(filters.tags(), &["demo"]);
    assert_eq!(filters.capabilities(), &["chat"]);
}

#[test]
fn from_wire_absent_fields_become_empty_vecs() {
    let wire = SearchFiltersWire {
        media_type: None,
        tags: Some(vec!["coding".to_owned()]),
        capabilities: None,
    };
    let filters = SearchFilters::from_wire(Some(wire));
    assert!(filters.media_types().is_empty());
    assert_eq!(filters.tags(), &["coding"]);
    assert!(filters.capabilities().is_empty());
}
