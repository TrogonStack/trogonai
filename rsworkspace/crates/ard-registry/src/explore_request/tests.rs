use ard_catalog::{
    ExploreFacetRequestWire, ExploreQueryWire, ExploreRequestWire, ExploreResultTypeWire, SearchFiltersWire,
};

use super::ValidatedExploreRequest;

fn empty_result_type() -> ExploreResultTypeWire {
    ExploreResultTypeWire { facets: vec![] }
}

#[test]
fn maps_text_filters_and_facets() {
    let wire = ExploreRequestWire {
        query: Some(ExploreQueryWire {
            text: Some("assistant".to_owned()),
            filter: Some(SearchFiltersWire {
                media_type: Some(vec!["application/a2a-agent-card+json".to_owned()]),
                tags: None,
                capabilities: None,
            }),
        }),
        result_type: ExploreResultTypeWire {
            facets: vec![
                ExploreFacetRequestWire {
                    field: "type".to_owned(),
                },
                ExploreFacetRequestWire {
                    field: "tags".to_owned(),
                },
            ],
        },
    };
    let request = ValidatedExploreRequest::from_wire(wire);
    assert_eq!(request.text(), Some("assistant"));
    assert_eq!(request.filters().media_types(), &["application/a2a-agent-card+json"]);
    assert_eq!(request.facet_fields(), &["type", "tags"]);
}

#[test]
fn whitespace_text_becomes_none() {
    let wire = ExploreRequestWire {
        query: Some(ExploreQueryWire {
            text: Some("   ".to_owned()),
            filter: None,
        }),
        result_type: empty_result_type(),
    };
    let request = ValidatedExploreRequest::from_wire(wire);
    assert_eq!(request.text(), None);
}

#[test]
fn empty_text_becomes_none() {
    let wire = ExploreRequestWire {
        query: Some(ExploreQueryWire {
            text: Some(String::new()),
            filter: None,
        }),
        result_type: empty_result_type(),
    };
    let request = ValidatedExploreRequest::from_wire(wire);
    assert_eq!(request.text(), None);
}

#[test]
fn absent_query_yields_empty_filters_and_no_text() {
    let wire = ExploreRequestWire {
        query: None,
        result_type: empty_result_type(),
    };
    let request = ValidatedExploreRequest::from_wire(wire);
    assert_eq!(request.text(), None);
    assert!(request.filters().is_empty());
    assert!(request.facet_fields().is_empty());
}
