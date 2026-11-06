#![allow(clippy::expect_used)]

use std::error::Error;

use super::*;

#[test]
fn operational_error_retains_its_typed_cause_without_exposing_it_in_diagnostics() {
    let error = RunnerError::Input(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "canary-private-controller-location",
    ));
    let source = error.source().expect("typed operational cause");
    assert_eq!(
        source.downcast_ref::<io::Error>().expect("original I/O error").kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(source.to_string().contains("canary-private-controller-location"));
    for diagnostic in [error.to_string(), format!("{error:?}"), format!("{error:#?}")] {
        assert!(!diagnostic.contains("canary-private-controller-location"));
        assert!(!diagnostic.contains("PermissionDenied"));
    }
}
