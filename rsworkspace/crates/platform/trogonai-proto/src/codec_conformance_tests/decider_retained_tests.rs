use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, MessageView, OwnedView};
use serde_json::json;

use super::assert_json_codec;
use super::retained_fixture::retained_detail;
use crate::decider::v1::{
    DecideRequest, DecideRequestOwnedView, DecideRequestView, DecideResponse, DecideResponseOwnedView,
    DecideResponseView, DecidedEvent, DecidedEventOwnedView, DecidedEventView,
};
use crate::scheduler::schedules::v1::{PauseSchedule, SchedulePaused, ScheduleResumed};

#[test]
fn request_retention_keeps_opaque_command_and_idempotency_precondition() {
    retained_detail!(
        DecideRequest,
        DecideRequestOwnedView,
        DecideRequestView<'static>,
        json!({
            "command": {"@type": PauseSchedule::TYPE_URL, "value": "CgZiYWNrdXA="},
            "commandId": "command-7", "expectedRevision": "18446744073709551615"
        }),
        |handle| {
            assert_eq!(handle.command_id(), Some("command-7"));
            assert_eq!(handle.expected_revision(), Some(u64::MAX));
            let command = handle.command().as_option().expect("command");
            assert_eq!(command.type_url, PauseSchedule::TYPE_URL);
            assert_eq!(command.value, b"\x0a\x06backup");
            let owned = handle.to_owned_message();
            assert_eq!(
                owned
                    .command
                    .unpack_if::<PauseSchedule>(PauseSchedule::TYPE_URL)
                    .expect("typed command"),
                Some(PauseSchedule {
                    schedule_id: "backup".to_owned()
                })
            );
        }
    );
}

#[test]
fn decided_event_retention_keeps_deduplication_identity_with_payload() {
    retained_detail!(
        DecidedEvent,
        DecidedEventOwnedView,
        DecidedEventView<'static>,
        json!({
            "id": "event-7", "event": {"@type": SchedulePaused::TYPE_URL, "value": "CgZiYWNrdXA="}
        }),
        |handle| {
            assert_eq!(handle.id(), "event-7");
            let event = handle.event().as_option().expect("event");
            assert_eq!(event.type_url, SchedulePaused::TYPE_URL);
            assert_eq!(event.value, b"\x0a\x06backup");
            assert_eq!(
                handle
                    .to_owned_message()
                    .event
                    .unpack_if::<SchedulePaused>(SchedulePaused::TYPE_URL)
                    .expect("typed event"),
                Some(SchedulePaused {
                    schedule_id: "backup".to_owned()
                })
            );
        }
    );
}

#[test]
fn response_retention_keeps_append_order_distinct_from_batch_position() {
    retained_detail!(
        DecideResponse,
        DecideResponseOwnedView,
        DecideResponseView<'static>,
        json!({
            "streamPosition": "9007199254740993", "events": [
                {"id": "event-7", "event": {"@type": SchedulePaused::TYPE_URL, "value": "CgZiYWNrdXA="}},
                {"id": "event-8", "event": {"@type": ScheduleResumed::TYPE_URL, "value": "CgZiYWNrdXA="}}
            ]
        }),
        |handle| {
            assert_eq!(handle.stream_position(), 9_007_199_254_740_993);
            let events = &**handle.events();
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].id, "event-7");
            assert_eq!(events[1].id, "event-8");
            assert_eq!(
                events[0].event.as_option().expect("first event").type_url,
                SchedulePaused::TYPE_URL
            );
            assert_eq!(
                events[1].event.as_option().expect("second event").type_url,
                ScheduleResumed::TYPE_URL
            );
        }
    );
}

#[test]
fn request_json_preserves_present_empty_identity_and_zero_revision() {
    let request = assert_json_codec::<DecideRequest>(json!({"command": {}, "commandId": "", "expectedRevision": "0"}));
    assert_eq!(request.command_id, Some(String::new()));
    assert_eq!(request.expected_revision, Some(0));
    let nulls: DecideRequest =
        serde_json::from_value(json!({"command": null, "command_id": null, "expected_revision": null,
        "future": {"nested": [1, true]}}))
        .expect("null optional request fields");
    assert_eq!(nulls, DecideRequest::default());
    let aliases: DecideRequest =
        serde_json::from_value(json!({"command_id": "command-8", "expected_revision": 42})).expect("request aliases");
    assert_eq!(aliases.command_id.as_deref(), Some("command-8"));
    assert_eq!(aliases.expected_revision, Some(42));
    for input in [
        r#"{"expectedRevision":"18446744073709551616"}"#,
        r#"{"expectedRevision":-1}"#,
        r#"{"expectedRevision":"1","expected_revision":"2"}"#,
        r#"{"commandId":"one","command_id":"two"}"#,
        r#"{"command":{"@type":"type.googleapis.com/example.Unknown","value":"!"}}"#,
    ] {
        assert!(
            serde_json::from_str::<DecideRequest>(input).is_err(),
            "invalid request {input}"
        );
    }
}

#[test]
fn response_json_keeps_required_zero_and_treats_null_event_list_as_empty() {
    assert_json_codec::<DecideResponse>(json!({"streamPosition": "0"}));
    for input in [
        json!({"stream_position": 0, "events": null}),
        json!({"streamPosition": "0", "events": []}),
    ] {
        let response: DecideResponse = serde_json::from_value(input).expect("empty accepted event batch");
        assert_eq!(response.stream_position, 0);
        assert!(response.events.is_empty());
    }
    for input in [
        r#"{"events":[null]}"#,
        r#"{"streamPosition":-1}"#,
        r#"{"streamPosition":"1","stream_position":"2"}"#,
    ] {
        assert!(
            serde_json::from_str::<DecideResponse>(input).is_err(),
            "invalid response {input}"
        );
    }
}

#[test]
fn decider_required_presence_tracks_tags_even_when_values_are_empty() {
    let absent = DecideRequest::decode_view(b"").expect("absent command");
    let present = DecideRequest::decode_view(b"\x0a\x00").expect("present empty command");
    assert!(!absent.has_command());
    assert!(present.has_command());
    let absent = DecidedEvent::decode_view(b"").expect("absent event");
    let present = DecidedEvent::decode_view(b"\x0a\x00\x12\x00").expect("present empty event");
    assert!(!absent.has_id());
    assert!(!absent.has_event());
    assert!(present.has_id());
    assert!(present.has_event());
    let absent = DecideResponse::decode_view(b"").expect("absent batch position");
    let present = DecideResponse::decode_view(b"\x08\x00").expect("explicit zero position");
    assert!(!absent.has_stream_position());
    assert!(present.has_stream_position());
    assert_eq!(
        absent.to_owned_message().expect("absent response"),
        present.to_owned_message().expect("present response")
    );
}
