use serde_json::json;

use super::*;
use crate::catalog_host::CatalogHostError;

#[test]
fn valid_host_converts_ok() {
    let wire = CatalogHostWire(json!({"displayName": "Acme Registry"}));
    let host = wire.into_domain().unwrap();
    assert_eq!(host.as_value()["displayName"], "Acme Registry");
}

#[test]
fn missing_display_name_errors() {
    let wire = CatalogHostWire(json!({}));
    assert_eq!(wire.into_domain(), Err(CatalogHostError::MissingDisplayName));
}

#[test]
fn blank_display_name_errors() {
    let wire = CatalogHostWire(json!({"displayName": "   "}));
    assert_eq!(wire.into_domain(), Err(CatalogHostError::MissingDisplayName));
}

#[test]
fn non_object_errors() {
    let wire = CatalogHostWire(json!("not-an-object"));
    assert_eq!(wire.into_domain(), Err(CatalogHostError::NotObject));
}

#[test]
fn try_from_valid_host_ok() {
    let wire = CatalogHostWire(json!({"displayName": "Peer Registry"}));
    let host = CatalogHost::try_from(wire).unwrap();
    assert_eq!(host.as_value()["displayName"], "Peer Registry");
}

#[test]
fn round_trip_preserves_unknown_extra_fields() {
    let wire = CatalogHostWire(json!({"displayName": "X", "custom": "y"}));
    let host = wire.clone().into_domain().unwrap();
    let back = CatalogHostWire(host.into_value());
    assert_eq!(back.0["displayName"], "X");
    assert_eq!(back.0["custom"], "y");
}
