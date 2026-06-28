use ard_catalog::{FederationMode, SearchQueryWire, SearchRequestWire};

use super::{SearchRequestError, ValidatedSearchRequest};

#[test]
fn rejects_empty_query() {
    let wire = SearchRequestWire {
        query: SearchQueryWire {
            text: "   ".to_owned(),
            filter: None,
        },
        page_size: None,
        page_token: None,
        federation: FederationMode::None,
    };
    assert_eq!(
        ValidatedSearchRequest::try_from_wire(wire, 0),
        Err(SearchRequestError::EmptyQuery)
    );
}

#[test]
fn defaults_limit() {
    let wire = SearchRequestWire {
        query: SearchQueryWire {
            text: "assistant".to_owned(),
            filter: None,
        },
        page_size: None,
        page_token: None,
        federation: FederationMode::None,
    };
    let request = ValidatedSearchRequest::try_from_wire(wire, 0).unwrap();
    assert_eq!(request.limit(), ValidatedSearchRequest::DEFAULT_LIMIT);
}
