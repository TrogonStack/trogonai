use buffa::{DecodeError, Message, MessageField};
use buffa_types::google::protobuf::Any;

use super::{assert_collection_limit, assert_malformed, assert_wire_codec};
use crate::decider::v1::{DecideRequest, DecideResponse, DecidedEvent};
use crate::scheduler::schedules::v1::{PauseSchedule, SchedulePaused, ScheduleResumed};

#[test]
fn decide_request_preserves_command_type_and_optional_concurrency_precondition() {
    let command = PauseSchedule {
        schedule_id: "backup".to_owned(),
    };
    let mut request = DecideRequest {
        command: MessageField::some(Any::pack(&command, PauseSchedule::TYPE_URL)),
        command_id: None,
        expected_revision: None,
    };
    let absent = request.encode_to_vec();
    assert_wire_codec(&absent, &request);
    request.command_id = Some(String::new());
    request.expected_revision = Some(0);
    let present = request.encode_to_vec();
    assert_eq!(&present[absent.len()..], &[0x12, 0x00, 0x18, 0x00]);
    assert_wire_codec(&present, &request);
    request.command_id = Some("command-7".to_owned());
    request.expected_revision = Some(u64::MAX);
    let wire = request.encode_to_vec();
    assert_wire_codec(&wire, &request);
    let decoded = DecideRequest::decode_from_slice(&wire).expect("request decode");
    let diagnostic = format!("{decoded:?}");
    assert!(diagnostic.contains("DecideRequest"));
    assert!(diagnostic.contains("command_id: Some(\"command-7\")"));
    assert!(diagnostic.contains("expected_revision: Some(18446744073709551615)"));
    assert!(diagnostic.contains(PauseSchedule::TYPE_URL));
    assert_eq!(
        decoded
            .command
            .unpack_if::<PauseSchedule>(PauseSchedule::TYPE_URL)
            .expect("typed command"),
        Some(command)
    );
}

#[test]
fn decide_response_preserves_event_order_and_stream_position_when_merged() {
    let paused = SchedulePaused {
        schedule_id: "backup".to_owned(),
    };
    let resumed = ScheduleResumed {
        schedule_id: "backup".to_owned(),
    };
    let first_event = DecidedEvent {
        id: "event-7".to_owned(),
        event: MessageField::some(Any::pack(&paused, SchedulePaused::TYPE_URL)),
    };
    let second_event = DecidedEvent {
        id: "event-8".to_owned(),
        event: MessageField::some(Any::pack(&resumed, ScheduleResumed::TYPE_URL)),
    };
    assert_wire_codec(&first_event.encode_to_vec(), &first_event);
    assert_wire_codec(&second_event.encode_to_vec(), &second_event);
    let first = DecideResponse {
        stream_position: 7,
        events: vec![first_event.clone()],
    };
    let second = DecideResponse {
        stream_position: u64::MAX,
        events: vec![second_event.clone()],
    };
    let expected = DecideResponse {
        stream_position: u64::MAX,
        events: vec![first_event, second_event],
    };
    let wire = [first.encode_to_vec(), second.encode_to_vec()].concat();
    assert_wire_codec(&wire, &expected);
    let decoded = DecideResponse::decode_from_slice(&wire).expect("response decode");
    let diagnostic = format!("{decoded:?}");
    assert!(diagnostic.contains("DecideResponse"));
    assert!(diagnostic.contains("stream_position: 18446744073709551615"));
    assert!(diagnostic.contains("DecidedEvent { id: \"event-7\""));
    assert!(diagnostic.contains("DecidedEvent { id: \"event-8\""));
    assert!(diagnostic.contains(SchedulePaused::TYPE_URL));
    assert!(diagnostic.contains(ScheduleResumed::TYPE_URL));
    assert_eq!(
        decoded.events[0]
            .event
            .unpack_if::<SchedulePaused>(SchedulePaused::TYPE_URL)
            .expect("paused event"),
        Some(paused)
    );
    assert_eq!(
        decoded.events[1]
            .event
            .unpack_if::<ScheduleResumed>(ScheduleResumed::TYPE_URL)
            .expect("resumed event"),
        Some(resumed)
    );
}

#[test]
fn malformed_decider_any_type_url_and_nested_event_are_rejected() {
    assert_malformed::<DecideRequest>(b"\x0a\x03\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<DecidedEvent>(b"\x12\x03\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<DecideResponse>(b"\x12\x04\x0a\x01x", DecodeError::UnexpectedEof);
}

#[test]
fn empty_decided_events_still_consume_collection_memory() {
    assert_collection_limit::<DecideResponse>(b"\x12\x00");
}
