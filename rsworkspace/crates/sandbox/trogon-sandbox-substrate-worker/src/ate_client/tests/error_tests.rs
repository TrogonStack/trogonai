use tonic::Code;

use crate::ate_client::AteError;

fn classify(code: Code) -> AteError {
    AteError::from(tonic::Status::new(code, "boom"))
}

#[test]
fn already_exists_is_a_signal_not_a_failure() {
    assert_eq!(
        classify(Code::AlreadyExists),
        AteError::AlreadyExists {
            message: "boom".to_string()
        }
    );
    assert!(!classify(Code::AlreadyExists).is_retryable());
}

#[test]
fn not_found_is_distinguished_so_cancel_can_treat_it_as_success() {
    assert!(matches!(classify(Code::NotFound), AteError::NotFound { .. }));
    assert!(!classify(Code::NotFound).is_retryable());
}

#[test]
fn control_plane_flapping_is_retryable() {
    for code in [Code::Unavailable, Code::DeadlineExceeded, Code::Aborted] {
        let err = classify(code);
        assert!(matches!(err, AteError::Unavailable { .. }), "{code:?}");
        assert!(err.is_retryable(), "{code:?}");
    }
}

#[test]
fn a_not_yet_ready_precondition_is_retryable_within_a_deadline() {
    assert!(matches!(
        classify(Code::FailedPrecondition),
        AteError::FailedPrecondition { .. }
    ));
    assert!(classify(Code::FailedPrecondition).is_retryable());
}

#[test]
fn a_full_cluster_is_retryable() {
    assert!(matches!(
        classify(Code::ResourceExhausted),
        AteError::ResourceExhausted { .. }
    ));
    assert!(classify(Code::ResourceExhausted).is_retryable());
}

#[test]
fn an_identity_fault_is_never_retried_and_never_blamed_on_the_tenant() {
    for code in [Code::PermissionDenied, Code::Unauthenticated] {
        let err = classify(code);
        assert!(matches!(err, AteError::PermissionDenied { .. }), "{code:?}");
        assert!(!err.is_retryable(), "{code:?}");
    }
}

#[test]
fn our_own_bad_request_fails_fast() {
    let err = classify(Code::InvalidArgument);
    assert!(matches!(err, AteError::InvalidArgument { .. }));
    assert!(!err.is_retryable());
}

#[test]
fn an_unmapped_code_keeps_the_code_in_the_message() {
    let err = classify(Code::Internal);
    assert!(matches!(err, AteError::Other { .. }));
    assert!(err.message().contains("Internal"), "{err}");
    assert!(!err.is_retryable());
}

#[test]
fn every_variant_exposes_its_message() {
    for code in [
        Code::AlreadyExists,
        Code::NotFound,
        Code::FailedPrecondition,
        Code::ResourceExhausted,
        Code::PermissionDenied,
        Code::Unavailable,
        Code::InvalidArgument,
        Code::Internal,
    ] {
        let err = classify(code);
        assert!(err.message().ends_with("boom"), "{code:?}: {}", err.message());
    }
}
