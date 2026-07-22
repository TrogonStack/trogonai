use ard_catalog::{FederationMode, SearchQueryWire, SearchRequestWire};

use super::{SearchRequestError, ValidatedSearchRequest};
use crate::registry_error::RegistryError;

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
    assert!(matches!(
        ValidatedSearchRequest::try_from_wire(wire),
        Err(RegistryError::SearchRequest(SearchRequestError::EmptyQuery))
    ));
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
    let request = ValidatedSearchRequest::try_from_wire(wire).unwrap();
    assert_eq!(request.limit(), ValidatedSearchRequest::DEFAULT_LIMIT);
}

#[test]
fn rejects_limit_exceeding_max() {
    let wire = SearchRequestWire {
        query: SearchQueryWire {
            text: "assistant".to_owned(),
            filter: None,
        },
        page_size: Some(ValidatedSearchRequest::MAX_LIMIT + 1),
        page_token: None,
        federation: FederationMode::None,
    };
    assert!(matches!(
        ValidatedSearchRequest::try_from_wire(wire),
        Err(RegistryError::SearchRequest(SearchRequestError::InvalidLimit { .. }))
    ));
}
