use std::error::Error;

use super::*;

#[test]
fn display_gateway() {
    assert_eq!(
        ImportGateError::Gateway("spicedb down".into()).to_string(),
        "federated discovery import gate: spicedb down"
    );
}

#[test]
fn source_none() {
    assert!(ImportGateError::Gateway("x".into()).source().is_none());
}
