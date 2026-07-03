use ard_catalog::ListAgentsQueryWire;

use super::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, ValidatedListAgentsQuery};
use crate::registry_error::RegistryError;

#[test]
fn defaults_page_size() {
    let query = ValidatedListAgentsQuery::try_from_wire(ListAgentsQueryWire {
        page_size: None,
        page_token: None,
        filters: None,
    })
    .unwrap();
    assert_eq!(query.page_size(), DEFAULT_PAGE_SIZE);
}

#[test]
fn rejects_page_size_zero() {
    let wire = ListAgentsQueryWire {
        page_size: Some(0),
        page_token: None,
        filters: None,
    };
    assert!(matches!(
        ValidatedListAgentsQuery::try_from_wire(wire),
        Err(RegistryError::InvalidPageSize { max: MAX_PAGE_SIZE })
    ));
}

#[test]
fn rejects_page_size_over_max() {
    let wire = ListAgentsQueryWire {
        page_size: Some(MAX_PAGE_SIZE + 1),
        page_token: None,
        filters: None,
    };
    assert!(matches!(
        ValidatedListAgentsQuery::try_from_wire(wire),
        Err(RegistryError::InvalidPageSize { max: MAX_PAGE_SIZE })
    ));
}
