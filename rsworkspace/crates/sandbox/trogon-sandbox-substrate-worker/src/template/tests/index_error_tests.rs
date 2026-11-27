use std::error::Error;

use crate::ate_client::AteError;
use crate::template::{TemplateError, TemplateIndexError};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_storage_failure_keeps_its_source() {
    let error = TemplateIndexError::storage(std::io::Error::other("disk full"));
    assert_eq!(error.to_string(), "template index storage failed: disk full");
    assert!(error.source().is_some());
}

#[test]
fn an_encode_failure_names_the_record() -> TestResult {
    let cause = serde_json::from_str::<u8>("x")
        .err()
        .ok_or("expected a parse failure")?;
    let error = TemplateIndexError::Encode(cause);
    assert!(error.source().is_some());
    Ok(())
}

#[test]
fn a_control_plane_error_becomes_a_backend_error() {
    let error = TemplateError::from(AteError::Unavailable { message: "down".into() });
    assert_eq!(
        error,
        TemplateError::Backend {
            message: "control plane unavailable: down".into()
        }
    );
    assert_eq!(
        error.to_string(),
        "substrate template error: control plane unavailable: down"
    );
}

#[test]
fn a_not_ready_error_names_the_image_and_the_reason() {
    let error = TemplateError::NotReady {
        image: "img".into(),
        reason: "pull access denied".into(),
    };
    assert_eq!(error.to_string(), "image img is not ready: pull access denied");
}
