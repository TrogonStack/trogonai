use trogon_std::NowV7;
use uuid::Uuid;

use super::*;

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[derive(Debug, Clone, Copy)]
struct FixedUuidGenerator(Uuid);

impl NowV7 for FixedUuidGenerator {
    fn now_v7(&self) -> Uuid {
        self.0
    }
}

#[test]
fn spec_precondition_wins_over_override_and_position() {
    let resolved = resolve_write_precondition(
        Some(host::WritePrecondition::NoStream),
        Some(StreamWritePrecondition::Any),
        Some(position(7)),
    );
    assert_eq!(resolved, StreamWritePrecondition::NoStream);
}

#[test]
fn override_wins_over_observed_position() {
    let resolved = resolve_write_precondition(None, Some(StreamWritePrecondition::StreamExists), Some(position(7)));
    assert_eq!(resolved, StreamWritePrecondition::StreamExists);
}

#[test]
fn observed_position_is_the_fallback() {
    let resolved = resolve_write_precondition(None, None, Some(position(7)));
    assert_eq!(resolved, StreamWritePrecondition::At(position(7)));
}

#[test]
fn missing_position_falls_back_to_no_stream() {
    let resolved = resolve_write_precondition(None, None, None);
    assert_eq!(resolved, StreamWritePrecondition::NoStream);
}

#[test]
fn only_no_stream_uses_the_fast_path() {
    assert!(is_no_stream(Some(host::WritePrecondition::NoStream)));
    assert!(!is_no_stream(Some(host::WritePrecondition::Any)));
    assert!(!is_no_stream(Some(host::WritePrecondition::StreamExists)));
    assert!(!is_no_stream(None));
}

#[test]
fn wit_preconditions_map_onto_stream_preconditions() {
    assert_eq!(
        to_stream_write_precondition(host::WritePrecondition::Any),
        StreamWritePrecondition::Any
    );
    assert_eq!(
        to_stream_write_precondition(host::WritePrecondition::StreamExists),
        StreamWritePrecondition::StreamExists
    );
    assert_eq!(
        to_stream_write_precondition(host::WritePrecondition::NoStream),
        StreamWritePrecondition::NoStream
    );
}

#[test]
fn snapshot_at_or_behind_stream_is_accepted() {
    assert!(ensure_snapshot_not_ahead(position(3), Some(position(3))).is_ok());
    assert!(ensure_snapshot_not_ahead(position(3), Some(position(9))).is_ok());
}

#[test]
fn snapshot_ahead_of_stream_is_rejected() {
    let Err(error) = ensure_snapshot_not_ahead(position(9), Some(position(3))) else {
        panic!("expected snapshot ahead of stream error");
    };
    assert_eq!(error.snapshot_position, position(9));
    assert_eq!(error.stream_position, Some(position(3)));

    assert!(ensure_snapshot_not_ahead(position(1), None).is_err());
}

#[test]
fn encode_events_assigns_host_ids_and_headers() {
    let id = Uuid::now_v7();
    let headers = Headers::empty();
    let envelopes = vec![AnyEnvelope {
        type_: "test.v1.Happened".to_string(),
        payload: vec![1, 2, 3],
    }];

    let events = encode_events(envelopes, &headers, &FixedUuidGenerator(id));

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, EventId::new(id));
    assert_eq!(events[0].r#type, "test.v1.Happened");
    assert_eq!(events[0].content, vec![1, 2, 3]);
    assert_eq!(events[0].headers, headers);
}

#[test]
fn stream_events_project_onto_guest_envelopes() {
    let stream_event = trogon_decider_runtime::StreamEvent {
        stream_id: "backup".to_string(),
        event: Event {
            id: EventId::new(Uuid::now_v7()),
            r#type: "test.v1.Happened".to_string(),
            content: vec![4, 5, 6],
            headers: Headers::empty(),
        },
        stream_position: position(1),
        recorded_at: chrono::Utc::now(),
    };

    let envelopes = to_any_envelopes(vec![stream_event]);

    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].type_, "test.v1.Happened");
    assert_eq!(envelopes[0].payload, vec![4, 5, 6]);
}
