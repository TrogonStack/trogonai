use trogon_decider_runtime::{AdmissionLimit, AuthorizationDeniedError};

use super::*;

#[derive(Debug, thiserror::Error)]
#[error("the object store rejected the read")]
struct StoreRejectedError;

#[derive(Debug, thiserror::Error)]
#[error("jetstream is unreachable")]
struct StorageDownError(#[source] StoreRejectedError);

fn error_info_of(status: &Status) -> ErrorInfo {
    find_detail::<ErrorInfo>(status).expect("every decider status names its reason")
}

fn overloaded(limit: usize) -> OverloadedError {
    OverloadedError::new(AdmissionLimit::try_new(limit).expect("a positive limit"))
}

#[test]
fn every_fault_class_reports_a_canonical_code() {
    let classes = [
        (FaultClass::Unroutable, Code::UNIMPLEMENTED),
        (FaultClass::InvalidRequest, Code::INVALID_ARGUMENT),
        (FaultClass::UnsatisfiablePrecondition, Code::INVALID_ARGUMENT),
        (FaultClass::Conflict, Code::ABORTED),
        (FaultClass::Guest, Code::INTERNAL),
        (FaultClass::DeadlineExceeded, Code::DEADLINE_EXCEEDED),
        (FaultClass::Storage, Code::UNAVAILABLE),
        (FaultClass::Internal, Code::INTERNAL),
    ];

    for (class, expected) in classes {
        let status = faulted(class, &StorageDownError(StoreRejectedError));
        assert_eq!(
            status.code, expected as i32,
            "a caller branching on {class:?} branches on the code space every other service uses"
        );
    }
}

#[test]
fn two_classes_sharing_a_code_stay_distinguishable_by_reason() {
    let guest = faulted(FaultClass::Guest, &StoreRejectedError);
    let internal = faulted(FaultClass::Internal, &StoreRejectedError);

    assert_eq!(guest.code, internal.code, "both are INTERNAL to a caller");
    assert_ne!(
        error_info_of(&guest).reason,
        error_info_of(&internal).reason,
        "the guest and the host are answerable for these separately, so an operator has to tell them apart"
    );
}

#[test]
fn every_fault_reason_matches_the_error_info_format() {
    let classes = [
        FaultClass::Unroutable,
        FaultClass::InvalidRequest,
        FaultClass::UnsatisfiablePrecondition,
        FaultClass::Conflict,
        FaultClass::Guest,
        FaultClass::DeadlineExceeded,
        FaultClass::Storage,
        FaultClass::Internal,
    ];

    for class in classes {
        let reason = class.reason();
        assert!(
            reason.len() <= 63
                && reason
                    .chars()
                    .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'),
            "`{reason}` has to satisfy ErrorInfo.reason's UPPER_SNAKE_CASE contract to be readable as one"
        );
    }
}

#[test]
fn a_fault_carries_its_source_chain_as_ordered_debug_info() {
    let status = faulted(FaultClass::Storage, &StorageDownError(StoreRejectedError));

    let debug = find_detail::<DebugInfo>(&status).expect("a chained error carries its chain");
    assert_eq!(
        debug.stack_entries,
        vec!["the object store rejected the read".to_owned()],
        "the list's order is the chain's order, which is what numbering the entries used to encode"
    );
}

#[test]
fn a_fault_with_no_chain_carries_no_debug_info() {
    let status = faulted(FaultClass::Storage, &StoreRejectedError);

    assert!(
        find_detail::<DebugInfo>(&status).is_none(),
        "an empty detail says what an absent one says and costs a caller a decode to find out"
    );
}

#[test]
fn a_fault_is_attributed_to_the_host_that_raised_it() {
    let info = error_info_of(&faulted(FaultClass::Internal, &StoreRejectedError));

    assert_eq!(info.domain, DECIDER_ERROR_DOMAIN);
}

#[test]
fn a_rejection_is_a_failed_precondition_under_the_module_s_own_domain() {
    let status = rejected(
        "schedules",
        "schedule_already_exists",
        "schedule 'nightly' already exists".to_owned(),
        &[],
    );

    assert_eq!(
        status.code,
        Code::FAILED_PRECONDITION as i32,
        "the command is well formed and would succeed against a different stream state"
    );
    let info = error_info_of(&status);
    assert_eq!(info.reason, "schedule_already_exists");
    assert_eq!(
        info.domain, "schedules",
        "two modules choosing the same code stay distinguishable only if the domain names the module"
    );
}

#[test]
fn a_rejection_keeps_the_guest_chain_in_the_order_the_guest_built_it() {
    let status = rejected(
        "schedules",
        "schedule_already_exists",
        "schedule 'nightly' already exists".to_owned(),
        &[
            ("cause.0".to_owned(), "duplicate key".to_owned()),
            ("cause.1".to_owned(), "index violation".to_owned()),
        ],
    );

    let debug = find_detail::<DebugInfo>(&status).expect("the guest attached a chain");
    assert_eq!(
        debug.stack_entries,
        vec![
            "cause.0: duplicate key".to_owned(),
            "cause.1: index violation".to_owned()
        ],
        "the chain is all that survived the WIT boundary; a map would drop its order"
    );
}

#[test]
fn a_shed_command_carries_its_limit_as_a_number() {
    let status = shed(overloaded(32));

    assert_eq!(status.code, Code::RESOURCE_EXHAUSTED as i32);
    let quota = find_detail::<QuotaFailure>(&status).expect("a shed names the quota it contended for");
    let violation = quota.violations.first().expect("exactly one quota was violated");
    assert_eq!(
        violation.quota_value, 32,
        "a caller sizing a backoff reads a number, not a limit spelled out in a message"
    );
    assert_eq!(violation.quota_metric, CONCURRENCY_QUOTA_METRIC);
}

#[test]
fn a_shed_command_names_no_subject() {
    let status = shed(overloaded(1));

    let quota = find_detail::<QuotaFailure>(&status).expect("a shed names the quota it contended for");
    assert_eq!(
        quota.violations[0].subject, "",
        "the limit is the host's, so nothing about who asked determined that the answer was no"
    );
}

#[test]
fn a_missing_principal_is_unauthenticated_and_a_refusal_is_permission_denied() {
    let missing = denied(&UnauthorizedError::MissingPrincipal);
    let refused = denied(&UnauthorizedError::Denied(AuthorizationDeniedError::new(
        "decider.write is required",
    )));

    assert_eq!(
        missing.code,
        Code::UNAUTHENTICATED as i32,
        "the caller's next move is to present credentials"
    );
    assert_eq!(
        refused.code,
        Code::PERMISSION_DENIED as i32,
        "the caller's next move is to present different credentials"
    );
    assert_eq!(error_info_of(&missing).reason, "PRINCIPAL_MISSING");
    assert_eq!(error_info_of(&refused).reason, "PRINCIPAL_UNAUTHORIZED");
}

#[test]
fn a_denial_says_only_what_the_authorizer_said() {
    let status = denied(&UnauthorizedError::Denied(AuthorizationDeniedError::new(
        "decider.write is required",
    )));

    assert_eq!(
        status.message, "command denied for this principal: decider.write is required",
        "the host defines no denial vocabulary of its own"
    );
    assert!(
        find_detail::<DebugInfo>(&status).is_none(),
        "a denial names no internals: nothing was read, decided, or appended"
    );
}

#[test]
fn a_detail_is_found_wherever_it_sits_in_the_list() {
    let status = shed(overloaded(4));

    assert!(
        find_detail::<QuotaFailure>(&status).is_some() && find_detail::<ErrorInfo>(&status).is_some(),
        "`details` is an unordered repeated Any, so a reader that indexes into it reads a position \
         nothing promised"
    );
}
