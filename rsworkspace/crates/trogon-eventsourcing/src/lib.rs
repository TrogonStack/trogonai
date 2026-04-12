use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

mod snapshots;

pub use snapshots::{
    SnapshotChange, SnapshotSchemaVersion, SnapshotSchemaVersionError, SnapshotStoreConfig,
    SnapshotStoreError, StreamSnapshot, checkpoint_key, list_snapshots, load_snapshot,
    load_snapshot_map, maybe_advance_checkpoint, persist_snapshot_change, read_checkpoint,
    snapshot_key, write_checkpoint,
};

pub trait StreamEvent {
    fn stream_id(&self) -> &str;
}

pub trait SubjectEvent: StreamEvent {
    const SUBJECT_PREFIX: &'static str;
}

pub trait EventType {
    fn event_type(&self) -> &'static str;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventData<E, M = serde_json::Value> {
    pub event_id: String,
    pub event_type: String,
    pub data: E,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<M>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedEvent<E, M = serde_json::Value> {
    pub event_id: String,
    pub event_type: String,
    pub data: E,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<M>,
    pub stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_position: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_position: Option<u64>,
    pub recorded_at: DateTime<Utc>,
}

impl<E> EventData<E>
where
    E: EventType,
{
    pub fn new(event: E) -> Self {
        Self::with_metadata(event, None::<serde_json::Value>)
    }
}

impl<E, M> EventData<E, M>
where
    E: EventType,
{
    pub fn with_metadata(event: E, metadata: Option<M>) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: event.event_type().to_string(),
            data: event,
            metadata,
        }
    }

    pub fn record(
        self,
        stream_id: impl Into<String>,
        stream_position: Option<u64>,
        log_position: Option<u64>,
        recorded_at: DateTime<Utc>,
    ) -> RecordedEvent<E, M> {
        RecordedEvent {
            event_id: self.event_id,
            event_type: self.event_type,
            data: self.data,
            metadata: self.metadata,
            stream_id: stream_id.into(),
            stream_position,
            log_position,
            recorded_at,
        }
    }
}

impl<E, M> EventData<E, M>
where
    E: StreamEvent,
{
    pub fn stream_id(&self) -> &str {
        self.data.stream_id()
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }
}

impl<E, M> EventData<E, M>
where
    E: SubjectEvent,
{
    pub fn subject(&self) -> String {
        self.subject_with_prefix(E::SUBJECT_PREFIX)
    }
}

impl<E, M> EventData<E, M>
where
    E: DeserializeOwned,
    M: DeserializeOwned,
{
    pub fn decode(payload: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice::<Self>(payload)
    }
}

impl<E, M> RecordedEvent<E, M>
where
    E: StreamEvent,
{
    pub fn stream_id(&self) -> &str {
        self.data.stream_id()
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }
}

impl<E, M> RecordedEvent<E, M>
where
    E: SubjectEvent,
{
    pub fn subject(&self) -> String {
        self.subject_with_prefix(E::SUBJECT_PREFIX)
    }
}

impl<E, M> RecordedEvent<E, M>
where
    E: DeserializeOwned,
    M: DeserializeOwned,
{
    pub fn decode(payload: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice::<Self>(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestEvent {
        id: String,
        value: String,
    }

    impl StreamEvent for TestEvent {
        fn stream_id(&self) -> &str {
            &self.id
        }
    }

    impl SubjectEvent for TestEvent {
        const SUBJECT_PREFIX: &'static str = "events.test.";
    }

    impl EventType for TestEvent {
        fn event_type(&self) -> &'static str {
            "TestEvent"
        }
    }

    #[test]
    fn event_data_uses_event_traits() {
        let event = EventData::new(TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        });

        assert_eq!(event.stream_id(), "alpha");
        assert_eq!(event.event_type, "TestEvent");
        assert_eq!(event.subject(), "events.test.alpha");
        assert_eq!(
            event.subject_with_prefix("events.legacy."),
            "events.legacy.alpha"
        );
    }

    #[test]
    fn recorded_event_preserves_store_context() {
        let event = EventData::new(TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        });

        let recorded = event.record(
            "stream-alpha",
            Some(2),
            Some(10),
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.stream_id, "stream-alpha");
        assert_eq!(recorded.stream_position, Some(2));
        assert_eq!(recorded.log_position, Some(10));
        assert_eq!(recorded.subject(), "events.test.alpha");
    }

    #[test]
    fn event_data_and_recorded_event_decode_payloads() {
        let event = EventData::new(TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        });
        let event_payload = serde_json::to_vec(&event).unwrap();
        let decoded_event = EventData::<TestEvent>::decode(&event_payload).unwrap();
        assert_eq!(decoded_event, event);

        let recorded = event.record(
            "stream-alpha",
            None,
            Some(42),
            DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
        );
        let recorded_payload = serde_json::to_vec(&recorded).unwrap();
        let decoded_recorded = RecordedEvent::<TestEvent>::decode(&recorded_payload).unwrap();
        assert_eq!(decoded_recorded, recorded);
    }
}
