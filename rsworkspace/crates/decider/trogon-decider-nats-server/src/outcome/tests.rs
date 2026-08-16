use trogon_decider_nats::JetStreamStoreError;
use trogon_decider_runtime::{
    AdmissionLimit, AuthorizationDenied, EventId, Headers, StreamPosition, StreamWritePrecondition, UnauthorizedError,
};
use uuid::Uuid;

use super::*;
use crate::request::CommandRequestError;

#[derive(Debug, thiserror::Error)]
#[error("jetstream is unreachable")]
struct StorageDownError;

type StoreError = JetStreamStoreError<StorageDownError>;
type TestCommandError = WasmCommandError<StorageDownError, StorageDownError, StoreError>;

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

fn case(reply: &CommandReply) -> &CommandOutcomeCase {
    reply.outcome().outcome.as_ref().expect("every reply sets an arm")
}

fn faulted(reply: &CommandReply) -> &v1::CommandFaulted {
    match case(reply) {
        CommandOutcomeCase::Faulted(faulted) => faulted,
        other => panic!("expected a faulted reply, got {other:?}"),
    }
}

fn kind(reply: &CommandReply) -> &CommandFaultedKind {
    faulted(reply).kind.as_ref().expect("a fault always names its class")
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

    match case(&reply) {
        CommandOutcomeCase::Decided(accepted) => {
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
        other => panic!("expected a decided reply, got {other:?}"),
    }
}

#[test]
fn a_decided_reply_carries_the_event_payloads() {
    let reply = CommandReply::decided(&result(vec![event(1, "test.v1.Created", vec![7; 4096])]));

    match case(&reply) {
        CommandOutcomeCase::Decided(accepted) => {
            let event = accepted.events[0]
                .event
                .as_option()
                .expect("an event is always carried");
            assert_eq!(
                event.value.as_ref(),
                vec![7u8; 4096],
                "a caller warming a cache from its own write cannot do it from type names alone"
            );
        }
        other => panic!("expected a decided reply, got {other:?}"),
    }
}

#[test]
fn a_decided_reply_names_each_event_by_the_id_it_was_appended_under() {
    let appended = vec![
        event(1, "test.v1.Created", vec![1]),
        event(2, "test.v1.Scheduled", vec![2]),
    ];
    let reply = CommandReply::decided(&result(appended.clone()));

    match case(&reply) {
        CommandOutcomeCase::Decided(accepted) => {
            let ids: Vec<&str> = accepted.events.iter().map(|decided| decided.id.as_str()).collect();
            let expected: Vec<String> = appended.iter().map(|event| event.id.to_string()).collect();
            assert_eq!(
                ids, expected,
                "these ids are the stream's `Nats-Msg-Id`s; a caller that applies this reply and also \
                 tails the stream deduplicates on them"
            );
        }
        other => panic!("expected a decided reply, got {other:?}"),
    }
}

#[test]
fn a_decided_reply_strips_nothing_but_adds_the_any_type_url_prefix() {
    let reply = CommandReply::decided(&result(vec![event(
        1,
        "trogonai.scheduler.schedules.v1.ScheduleCreated",
        vec![],
    )]));

    match case(&reply) {
        CommandOutcomeCase::Decided(accepted) => {
            assert_eq!(
                accepted.events[0]
                    .event
                    .as_option()
                    .expect("an event is always carried")
                    .type_url,
                "type.googleapis.com/trogonai.scheduler.schedules.v1.ScheduleCreated",
                "the stream stores the bare full name and `Any` requires the prefix; the prefix is the \
                 whole of the difference between the two"
            );
        }
        other => panic!("expected a decided reply, got {other:?}"),
    }
}

#[test]
fn a_rejection_keeps_the_guest_code_message_and_details() {
    let reply = CommandReply::rejected(&guest_error());

    match case(&reply) {
        CommandOutcomeCase::Rejected(rejected) => {
            assert_eq!(rejected.code, "schedule_already_exists");
            assert_eq!(rejected.message, "schedule 'nightly' already exists");
            assert_eq!(rejected.details.len(), 1);
            assert_eq!(rejected.details[0].key, "cause.0");
        }
        other => panic!("expected a rejected reply, got {other:?}"),
    }
}

#[test]
fn a_rejection_is_not_a_fault() {
    let reply = CommandReply::from_command_error(&TestCommandError::Rejected(guest_error()));

    assert_eq!(
        reply.header_value(),
        "rejected",
        "a module refusing an invalid command is the decider pattern working, not a service error"
    );
}

#[test]
fn a_shed_command_carries_the_limit_it_contended_for() {
    let limit = AdmissionLimit::try_new(32).expect("a positive limit");
    let reply = CommandReply::from_command_error(&TestCommandError::Overloaded(OverloadedError::new(limit)));

    assert_eq!(reply.header_value(), "shed");
    match case(&reply) {
        CommandOutcomeCase::Shed(shed) => assert_eq!(
            shed.limit, 32,
            "a caller that knows the limit can size its backoff instead of guessing"
        ),
        other => panic!("expected a shed reply, got {other:?}"),
    }
}

#[test]
fn a_denied_command_carries_the_reason_the_authorizer_gave() {
    let denied = UnauthorizedError::Denied(AuthorizationDenied::new("missing claim orders.write"));
    let reply = CommandReply::from_command_error(&TestCommandError::Unauthorized(denied));

    assert_eq!(reply.header_value(), "denied");
    match case(&reply) {
        CommandOutcomeCase::Denied(denied) => assert_eq!(
            denied.reason, "command denied for this principal: missing claim orders.write",
            "the caller is told why it was refused, since the host defines no denial codes to branch on"
        ),
        other => panic!("expected a denied reply, got {other:?}"),
    }
}

#[test]
fn a_command_with_no_principal_is_denied_rather_than_faulted() {
    let reply = CommandReply::from_command_error(&TestCommandError::Unauthorized(UnauthorizedError::MissingPrincipal));

    assert_eq!(
        reply.header_value(),
        "denied",
        "an authorizer configured without a principal is a caller problem, not a broken host"
    );
}

#[test]
fn an_unroutable_command_type_is_a_fault_of_its_own_class() {
    let reply = CommandReply::unroutable(&StorageDownError);

    assert!(matches!(kind(&reply), CommandFaultedKind::Unroutable(_)));
}

#[test]
fn an_unreadable_envelope_is_an_invalid_request() {
    let reply = CommandReply::invalid_request(&CommandRequestError::ExpectedRevisionZero);

    assert!(matches!(kind(&reply), CommandFaultedKind::InvalidRequest(_)));
}

#[test]
fn a_write_conflict_under_an_append_is_a_conflict_not_a_storage_fault() {
    let reply = CommandReply::from_command_error(&TestCommandError::Append(write_conflict()));

    assert!(
        matches!(kind(&reply), CommandFaultedKind::Conflict(_)),
        "a retry replays the stream as it now stands, which is not the advice a storage fault gives"
    );
}

#[test]
fn an_append_that_is_not_a_conflict_stays_a_storage_fault() {
    let reply = CommandReply::from_command_error(&TestCommandError::Append(StoreError::Codec(StorageDownError)));

    assert!(matches!(kind(&reply), CommandFaultedKind::Storage(_)));
}

#[test]
fn a_stream_read_failure_is_a_storage_fault() {
    let reply = CommandReply::from_command_error(&TestCommandError::ReadStream(StorageDownError));

    assert!(matches!(kind(&reply), CommandFaultedKind::Storage(_)));
}

#[test]
fn a_snapshot_read_failure_is_a_storage_fault() {
    let reply = CommandReply::from_command_error(&TestCommandError::ReadSnapshot(StorageDownError));

    assert!(matches!(kind(&reply), CommandFaultedKind::Storage(_)));
}

#[test]
fn a_decision_with_no_events_is_a_guest_fault() {
    let reply = CommandReply::from_command_error(&TestCommandError::EmptyDecision);

    assert!(
        matches!(kind(&reply), CommandFaultedKind::Guest(_)),
        "the guest broke the WIT contract, so retrying repeats the same call"
    );
}

#[test]
fn a_guest_fault_keeps_the_details_the_guest_attached() {
    let reply = CommandReply::from_command_error(&TestCommandError::Faulted(guest_error()));

    assert!(matches!(kind(&reply), CommandFaultedKind::Guest(_)));
    assert_eq!(
        faulted(&reply).details.len(),
        1,
        "the guest's chain is all that survived the WIT boundary; dropping it here loses it for good"
    );
}

#[test]
fn every_reply_header_agrees_with_the_arm_it_summarizes() {
    let replies = [
        CommandReply::decided(&result(vec![event(1, "test.v1.Created", vec![1])])),
        CommandReply::rejected(&guest_error()),
        CommandReply::from_command_error(&TestCommandError::Overloaded(OverloadedError::new(
            AdmissionLimit::try_new(1).expect("a positive limit"),
        ))),
        CommandReply::from_command_error(&TestCommandError::EmptyDecision),
        CommandReply::from_command_error(&TestCommandError::Unauthorized(UnauthorizedError::MissingPrincipal)),
    ];

    for reply in &replies {
        let expected = match case(reply) {
            CommandOutcomeCase::Decided(_) => "decided",
            CommandOutcomeCase::Rejected(_) => "rejected",
            CommandOutcomeCase::Faulted(_) => "faulted",
            CommandOutcomeCase::Shed(_) => "shed",
            CommandOutcomeCase::Denied(_) => "denied",
        };

        assert_eq!(
            reply.header_value(),
            expected,
            "middleware meters on the header without decoding the body, so the two cannot disagree"
        );
    }
}

#[test]
fn a_host_fault_flattens_its_source_chain_into_ordered_details() {
    let reply = CommandReply::from_command_error(&TestCommandError::ReadStream(StorageDownError));

    let details = &faulted(&reply).details;
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].key, "cause.0");
    assert_eq!(details[0].value, "jetstream is unreachable");
}
