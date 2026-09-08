use futures::StreamExt;

use super::*;

#[test]
fn debug_fmt_is_non_exhaustive() {
    let _ = format!("{:?}", InMemoryKv::new());
    let _ = format!("{:?}", InMemoryExecution::new());
}

#[test]
fn distinct_stream_events_at_the_same_position_have_distinct_event_ids() {
    let first_id = "0198fa2f6d0a7b1a8cf9f762e73a1c01";
    let second_id = "0198fa2f6d0a7b1a8cf9f762e73a1c02";
    let first_event = v1::ScheduleEvent {
        event: Some(
            v1::SchedulePaused {
                schedule_id: first_id.to_string(),
            }
            .into(),
        ),
    };
    let second_event = v1::ScheduleEvent {
        event: Some(
            v1::SchedulePaused {
                schedule_id: second_id.to_string(),
            }
            .into(),
        ),
    };

    let first = stream_event(&first_event, first_id, 1);
    let second = stream_event(&second_event, second_id, 1);

    assert_ne!(first.event.id, second.event.id);
    assert_eq!(first.event.id, first.clone().event.id);
}

#[tokio::test]
async fn kv_get_create_update_and_keys_are_observable() {
    let kv = InMemoryKv::new();
    assert!(
        kv.get("0198fa2f6d0a7b1a8cf9f762e73a1c14".to_string())
            .await
            .unwrap()
            .is_none()
    );

    kv.create("v1.orders", Bytes::from_static(b"{}")).await.unwrap();
    assert_eq!(
        kv.get("v1.orders".to_string()).await.unwrap(),
        Some(Bytes::from_static(b"{}"))
    );

    let duplicate = kv.create("v1.orders", Bytes::from_static(b"x")).await;
    assert_eq!(duplicate.unwrap_err().kind(), kv::CreateErrorKind::AlreadyExists);

    kv.update("v1.orders", Bytes::from_static(b"v2"), 99).await.unwrap_err();
    kv.update("v1.orders", Bytes::from_static(b"v2"), 1).await.unwrap();

    let mut keys = kv.keys().await.unwrap();
    assert_eq!(keys.next().await.transpose().unwrap(), Some("v1.orders".to_string()));
    assert!(keys.next().await.is_none());
}

#[tokio::test]
async fn memory_event_store_supports_append_read_and_conflict_preconditions() {
    let store = MemoryEventStore::default();
    let stream_id = ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c01").unwrap();
    let first_event = foreign_stream_event(1).event;
    let second_event = foreign_stream_event(2).event;

    let first_append = store
        .append_stream(AppendStreamRequest {
            stream_id: &stream_id,
            stream_write_precondition: StreamWritePrecondition::Any,
            events: vec![first_event],
        })
        .await
        .unwrap();
    assert_eq!(first_append.stream_position, StreamPosition::try_new(1).unwrap());

    let second_append = store
        .append_stream(AppendStreamRequest {
            stream_id: &stream_id,
            stream_write_precondition: StreamWritePrecondition::StreamExists,
            events: vec![second_event.clone()],
        })
        .await
        .unwrap();
    assert_eq!(second_append.stream_position, StreamPosition::try_new(2).unwrap());

    let read = store
        .read_stream(ReadStreamRequest {
            stream_id: &stream_id,
            from: ReadFrom::Position(StreamPosition::try_new(2).unwrap()),
        })
        .await
        .unwrap();
    assert_eq!(read.current_position, Some(StreamPosition::try_new(2).unwrap()));
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].event, second_event);
    assert_eq!(read.events[0].stream_position, StreamPosition::try_new(2).unwrap());

    let conflict = store
        .append_stream(AppendStreamRequest {
            stream_id: &stream_id,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![foreign_stream_event(3).event],
        })
        .await;
    assert_eq!(conflict.unwrap_err(), MemoryEventStoreError::Conflict);
}
