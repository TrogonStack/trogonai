use trogon_decider_nats::JetStreamStoreError;
use trogon_decider_runtime::{
    AdmissionLimit, AuthorizationDeniedError, EventId, Headers, OverloadedError, PreconditionConflictError,
    RevisionAheadOfStream, SnapshotAheadOfStream, StreamPosition, StreamWritePrecondition, UnauthorizedError,
};
use trogonai_proto::google::rpc::{Code, DebugInfo, ErrorInfo};
use uuid::Uuid;

use super::*;
use crate::request::CommandRequestError;
use crate::status::find_detail;

#[derive(Debug, thiserror::Error)]
#[error("jetstream is unreachable")]
struct StorageDownError;

type StoreError = JetStreamStoreError<StorageDownError>;
type TestCommandError = WasmCommandError<StorageDownError, StorageDownError, StoreError>;

fn module() -> ModuleName {
    ModuleName::new("schedules").expect("a valid module name")
}

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test positions are non-zero")
}

fn result(events: Vec<Event>) -> WasmExecutionResult {
    WasmExecutionResult {
        stream_position: position(7),
        events,
    }
}

/// One appended event, named the way the stream names it: a bare protobuf full
/// name, with no `Any` type URL prefix.
fn event(id: u128, type_: &str, payload: Vec<u8>) -> Event {
    Event {
        id: EventId::new(Uuid::from_u128(id)),
        r#type: type_.to_owned(),
        content: payload,
        headers: Headers::default(),
    }
}

/// What an accepted command's response carries, for the one reply that has one.
fn accepted(reply: &CommandReply) -> &v1::DecideResponse {
    reply.response().expect("this reply is a response, not a service error")
}

/// The `Status` of a reply that ADR#0016 makes a service error.
fn faulted(reply: &CommandReply) -> &Status {
    reply
        .service_error_status()
        .expect("this reply is a service error, not a response")
}

/// The reason a fault reports, which is what tells two classes sharing one code
/// apart.
fn fault_reason(reply: &CommandReply) -> String {
    find_detail::<ErrorInfo>(faulted(reply))
        .expect("every decider status names its reason")
        .reason
}

fn write_conflict() -> StoreError {
    StoreError::OptimisticConcurrencyConflict(OptimisticConcurrencyConflictError::WithPosition {
        stream_id: "schedule-1".to_owned(),
        expected: StreamWritePrecondition::At(position(4)),
        current_position: position(5),
    })
}

fn guest_error() -> GuestDomainError {
    GuestDomainError {
        code: "schedule_already_exists".to_owned(),
        message: "schedule 'nightly' already exists".to_owned(),
        details: vec![("cause.0".to_owned(), "duplicate key".to_owned())],
    }
}

#[test]
fn a_decided_command_reports_its_position_and_its_events_in_order() {
    let reply = CommandReply::decided(&result(vec![
        event(1, "test.v1.Created", vec![1]),
        event(2, "test.v1.Scheduled", vec![2]),
    ]));

    let accepted = accepted(&reply);
    assert_eq!(accepted.stream_position, 7);
    let types: Vec<&str> = accepted
        .events
        .iter()
        .map(|decided| {
            decided
                .event
                .as_option()
                .expect("an event is always carried")
                .type_url
                .as_str()
        })
        .collect();
    assert_eq!(
        types,
        vec![
            "type.googleapis.com/test.v1.Created",
            "type.googleapis.com/test.v1.Scheduled",
        ],
        "append order is the only order a caller folding these can rely on"
    );
}

#[test]
fn a_decided_reply_carries_the_event_payloads() {
    let reply = CommandReply::decided(&result(vec![event(1, "test.v1.Created", vec![7; 4096])]));

    let event = accepted(&reply).events[0]
        .event
        .as_option()
        .expect("an event is always carried");
    assert_eq!(
        event.value.as_ref(),
        vec![7u8; 4096],
        "a caller warming a cache from its own write cannot do it from type names alone"
    );
}

#[test]
fn a_decided_reply_names_each_event_by_the_id_it_was_appended_under() {
    let appended = vec![
        event(1, "test.v1.Created", vec![1]),
        event(2, "test.v1.Scheduled", vec![2]),
    ];
    let reply = CommandReply::decided(&result(appended.clone()));

    let ids: Vec<&str> = accepted(&reply)
        .events
        .iter()
        .map(|decided| decided.id.as_str())
        .collect();
    let expected: Vec<String> = appended.iter().map(|event| event.id.to_string()).collect();
    assert_eq!(
        ids, expected,
        "these ids are the stream's `Nats-Msg-Id`s; a caller that applies this reply and also \
         tails the stream deduplicates on them"
    );
}

#[test]
fn a_decided_reply_strips_nothing_but_adds_the_any_type_url_prefix() {
    let reply = CommandReply::decided(&result(vec![event(
        1,
        "trogonai.scheduler.schedules.v1.ScheduleCreated",
        vec![],
    )]));

    assert_eq!(
        accepted(&reply).events[0]
            .event
            .as_option()
            .expect("an event is always carried")
            .type_url,
        "type.googleapis.com/trogonai.scheduler.schedules.v1.ScheduleCreated",
        "the stream stores the bare full name and `Any` requires the prefix; the prefix is the \
         whole of the difference between the two"
    );
}

#[test]
fn a_rejection_keeps_the_guest_code_message_and_details() {
    let reply = CommandReply::rejected(&module(), &guest_error());
    let status = faulted(&reply);

    let info = find_detail::<ErrorInfo>(status).expect("every decider status names its reason");
    assert_eq!(info.reason, "schedule_already_exists");
    assert_eq!(info.domain, "schedules");
    assert_eq!(status.message, "schedule 'nightly' already exists");
    let debug = find_detail::<DebugInfo>(status).expect("the guest attached a chain");
    assert_eq!(debug.stack_entries, vec!["cause.0: duplicate key".to_owned()]);
}

#[test]
fn a_rejection_reports_under_the_module_rather_than_under_the_host() {
    let reply = CommandReply::from_command_error(&module(), &TestCommandError::Rejected(guest_error()));

    assert_eq!(reply.decision(), DecisionOutcome::Rejected);
    assert_eq!(
        faulted(&reply).code,
        Code::FAILED_PRECONDITION as i32,
        "the command is well-formed and would succeed against a different stream state, which is the \
         distinction `google.rpc.Code` draws between this and INVALID_ARGUMENT"
    );
    assert_eq!(
        find_detail::<ErrorInfo>(faulted(&reply))
            .expect("every decider status names its reason")
            .domain,
        module().as_str(),
        "the host reporting a module's code under its own domain would make two modules choosing the \
         same code indistinguishable"
    );
}

#[test]
fn a_shed_command_is_a_service_error_naming_the_quota() {
    let limit = AdmissionLimit::try_new(32).expect("a positive limit");
    let reply = CommandReply::from_command_error(&module(), &TestCommandError::Overloaded(OverloadedError::new(limit)));

    assert_eq!(reply.decision(), DecisionOutcome::Shed);
    assert_eq!(
        faulted(&reply).code,
        Code::RESOURCE_EXHAUSTED as i32,
        "a command admission control never started is one the host did not execute, so it answers on the error channel"
    );
}

#[test]
fn a_denied_command_is_a_service_error() {
    let denied = UnauthorizedError::Denied(AuthorizationDeniedError::new("missing claim orders.write"));
    let reply = CommandReply::from_command_error(&module(), &TestCommandError::Unauthorized(denied));

    assert_eq!(reply.decision(), DecisionOutcome::Denied);
    assert_eq!(
        faulted(&reply).code,
        Code::PERMISSION_DENIED as i32,
        "the principal was identified and refused, which is not the same as not being identified"
    );
}

#[test]
fn a_command_with_no_principal_is_unauthenticated_rather_than_permission_denied() {
    let reply = CommandReply::from_command_error(
        &module(),
        &TestCommandError::Unauthorized(UnauthorizedError::MissingPrincipal),
    );

    assert_eq!(
        reply.decision(),
        DecisionOutcome::Denied,
        "an authorizer configured without a principal is a caller problem, not a broken host"
    );
    assert_eq!(faulted(&reply).code, Code::UNAUTHENTICATED as i32);
}

#[test]
fn an_unroutable_command_type_is_a_fault_of_its_own_class() {
    let reply = CommandReply::unroutable(&StorageDownError);

    assert_eq!(fault_reason(&reply), FaultClass::Unroutable.reason());
    assert_eq!(faulted(&reply).code, Code::UNIMPLEMENTED as i32);
}

#[test]
fn an_unreadable_envelope_is_an_invalid_request() {
    let reply = CommandReply::invalid_request(&CommandRequestError::ExpectedRevisionZero);

    assert_eq!(fault_reason(&reply), FaultClass::InvalidRequest.reason());
    assert_eq!(faulted(&reply).code, Code::INVALID_ARGUMENT as i32);
}

#[test]
fn a_write_conflict_under_an_append_is_a_conflict_not_a_storage_fault() {
    let reply = CommandReply::from_command_error(&module(), &TestCommandError::Append(write_conflict()));

    assert_eq!(
        fault_reason(&reply),
        FaultClass::Conflict.reason(),
        "a retry replays the stream as it now stands, which is not the advice a storage fault gives"
    );
    assert_eq!(faulted(&reply).code, Code::ABORTED as i32);
}

#[test]
fn a_fabricated_expected_revision_is_not_reported_as_a_retryable_conflict() {
    let reply = CommandReply::from_command_error(
        &module(),
        &TestCommandError::PreconditionConflict(PreconditionConflictError::RevisionAheadOfStream(
            RevisionAheadOfStream {
                expected: position(9),
                observed: Some(position(5)),
            },
        )),
    );

    assert_eq!(
        fault_reason(&reply),
        FaultClass::UnsatisfiablePrecondition.reason(),
        "no stream state satisfies the revision, so the conflict advice would send a caller into a loop"
    );
    assert_eq!(faulted(&reply).code, Code::INVALID_ARGUMENT as i32);
    assert_ne!(faulted(&reply).code, Code::ABORTED as i32);
}

#[test]
fn a_create_command_carrying_a_revision_is_refused_as_the_caller_error_it_is() {
    let reply = CommandReply::from_command_error(
        &module(),
        &TestCommandError::PreconditionConflict(PreconditionConflictError::CreateWithRevision),
    );

    assert_eq!(fault_reason(&reply), FaultClass::UnsatisfiablePrecondition.reason());
    assert_eq!(faulted(&reply).code, Code::INVALID_ARGUMENT as i32);
}

#[test]
fn an_append_that_is_not_a_conflict_stays_a_storage_fault() {
    let reply = CommandReply::from_command_error(
        &module(),
        &TestCommandError::Append(StoreError::Codec(StorageDownError)),
    );

    assert_eq!(fault_reason(&reply), FaultClass::Storage.reason());
}

#[test]
fn a_stream_read_failure_is_a_storage_fault() {
    let reply = CommandReply::from_command_error(&module(), &TestCommandError::ReadStream(StorageDownError));

    assert_eq!(fault_reason(&reply), FaultClass::Storage.reason());
    assert_eq!(faulted(&reply).code, Code::UNAVAILABLE as i32);
}

#[test]
fn a_snapshot_read_failure_is_a_storage_fault() {
    let reply = CommandReply::from_command_error(&module(), &TestCommandError::ReadSnapshot(StorageDownError));

    assert_eq!(fault_reason(&reply), FaultClass::Storage.reason());
}

#[test]
fn a_decision_with_no_events_is_a_guest_fault() {
    let reply = CommandReply::from_command_error(&module(), &TestCommandError::EmptyDecision);

    assert_eq!(
        fault_reason(&reply),
        FaultClass::Guest.reason(),
        "the guest broke the WIT contract, so retrying repeats the same call"
    );
}

#[test]
fn a_guest_fault_keeps_the_details_the_guest_attached() {
    let reply = CommandReply::from_command_error(&module(), &TestCommandError::Faulted(guest_error()));

    assert_eq!(fault_reason(&reply), FaultClass::Guest.reason());
    let debug = find_detail::<DebugInfo>(faulted(&reply)).expect("the guest attached a chain");
    assert_eq!(
        debug.stack_entries.len(),
        1,
        "the guest's chain is all that survived the WIT boundary; dropping it here loses it for good"
    );
}

#[test]
fn a_host_fault_and_a_guest_fault_are_both_internal_but_not_the_same_reason() {
    let host = CommandReply::from_command_error(
        &module(),
        &TestCommandError::SnapshotAheadOfStream(SnapshotAheadOfStream {
            snapshot_position: position(9),
            stream_position: Some(position(4)),
        }),
    );
    let guest = CommandReply::from_command_error(&module(), &TestCommandError::EmptyDecision);

    assert_eq!(faulted(&host).code, faulted(&guest).code);
    assert_ne!(
        fault_reason(&host),
        fault_reason(&guest),
        "an operator paging on host faults must not be paged by a module's own bug"
    );
}

/// Every shape a reply can take, so a rule asserted below is asserted over all
/// of them rather than over the ones a reader happened to think of.
fn every_reply_shape() -> Vec<CommandReply> {
    vec![
        CommandReply::decided(&result(vec![event(1, "test.v1.Created", vec![1])])),
        CommandReply::rejected(&module(), &guest_error()),
        CommandReply::from_command_error(
            &module(),
            &TestCommandError::Overloaded(OverloadedError::new(
                AdmissionLimit::try_new(1).expect("a positive limit"),
            )),
        ),
        CommandReply::from_command_error(&module(), &TestCommandError::EmptyDecision),
        CommandReply::from_command_error(
            &module(),
            &TestCommandError::Unauthorized(UnauthorizedError::MissingPrincipal),
        ),
        CommandReply::unroutable(&StorageDownError),
        CommandReply::internal(&StorageDownError),
    ]
}

#[test]
fn a_reply_is_a_response_or_a_service_error_and_never_both() {
    for reply in every_reply_shape() {
        assert_ne!(
            reply.response().is_some(),
            reply.service_error_status().is_some(),
            "ADR#0016 discriminates on the error header alone, so a reply holding both bodies would encode as one of them and be read as the other: {reply:?}"
        );
    }
}

#[test]
fn only_an_accepted_command_answers_in_the_response_body() {
    for reply in every_reply_shape() {
        assert_eq!(
            reply.response().is_some(),
            reply.decision() == DecisionOutcome::Decided,
            "the response type describes what was decided and appended, so an outcome that decided and appended nothing has nothing to say in it: {reply:?}"
        );
    }
}

#[test]
fn no_reply_but_an_accepted_one_reports_ok() {
    for reply in every_reply_shape() {
        let Some(status) = reply.service_error_status() else {
            continue;
        };

        assert_ne!(
            status.code,
            Code::OK as i32,
            "`OK` on the status a caller reads after a refusal or a fault would contradict how it got there"
        );
    }
}

#[test]
fn a_reply_body_reaches_the_caller_as_the_body_the_host_built() {
    for reply in every_reply_shape() {
        let encoded = reply.encode();

        match reply.service_error_status() {
            Some(status) => assert_eq!(
                &Status::decode_from_slice(&encoded).expect("an error body is one complete Status"),
                status,
                "ADR#0016 promises a complete Status on the error channel, and a fragment is what a caller cannot tell from a truncated one"
            ),
            None => assert_eq!(
                &v1::DecideResponse::decode_from_slice(&encoded)
                    .expect("the host encodes what the descriptor tells the caller to decode"),
                accepted(&reply),
                "a body that did not survive the wire would report an outcome the host never reached"
            ),
        }
    }
}
