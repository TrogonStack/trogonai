use ard_catalog::SearchFiltersWire;

use super::SearchFilters;
use crate::registry_error::RegistryError;

#[test]
fn try_from_wire_none_is_empty() {
    let filters = SearchFilters::try_from_wire(None).unwrap();
    assert!(filters.is_empty());
    assert!(filters.media_types().is_empty());
    assert!(filters.tags().is_empty());
    assert!(filters.capabilities().is_empty());
}

#[test]
fn try_from_wire_maps_each_field() {
    let wire = SearchFiltersWire {
        media_type: Some(vec!["application/a2a-agent-card+json".to_owned()]),
        tags: Some(vec!["demo".to_owned()]),
        capabilities: Some(vec!["chat".to_owned()]),
    };
    let filters = SearchFilters::try_from_wire(Some(wire)).unwrap();
    assert!(!filters.is_empty());
    assert_eq!(filters.media_types().len(), 1);
    assert_eq!(filters.media_types()[0].as_str(), "application/a2a-agent-card+json");
    assert_eq!(filters.tags(), &["demo"]);
    assert_eq!(filters.capabilities(), &["chat"]);
}

#[test]
fn try_from_wire_absent_fields_become_empty_vecs() {
    let wire = SearchFiltersWire {
        media_type: None,
        tags: Some(vec!["coding".to_owned()]),
        capabilities: None,
    };
    let filters = SearchFilters::try_from_wire(Some(wire)).unwrap();
    assert!(filters.media_types().is_empty());
    assert_eq!(filters.tags(), &["coding"]);
    assert!(filters.capabilities().is_empty());
}

#[test]
fn rejects_invalid_media_type() {
    let wire = SearchFiltersWire {
        media_type: Some(vec!["not a media type".to_owned()]),
        tags: None,
        capabilities: None,
    };
    let result = SearchFilters::try_from_wire(Some(wire));
    assert!(matches!(result, Err(RegistryError::Filter(_))));
}

#[test]
fn accepts_valid_media_type() {
    let wire = SearchFiltersWire {
        media_type: Some(vec!["application/a2a-agent-card+json".to_owned()]),
        tags: None,
        capabilities: None,
    };
    let filters = SearchFilters::try_from_wire(Some(wire)).unwrap();
    assert_eq!(filters.media_types()[0].as_str(), "application/a2a-agent-card+json");
}
