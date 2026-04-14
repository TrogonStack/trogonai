use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

mod decision;
mod snapshots;

pub use decision::{Act, Decide, Decision, NonEmpty, StreamCommand, decide};
pub use snapshots::{
    Snapshot, SnapshotChange, SnapshotSchemaVersion, SnapshotSchemaVersionError,
    SnapshotStoreConfig, SnapshotStoreError, checkpoint_key, list_snapshots, load_snapshot,
    load_snapshot_map, maybe_advance_checkpoint, persist_snapshot_change, read_checkpoint,
    snapshot_key, write_checkpoint,
};

pub trait StreamEvent {
    fn stream_id(&self) -> &str;
}

pub trait EventType {
    fn event_type(&self) -> &'static str;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventData {
    pub event_id: String,
    pub event_type: String,
    pub stream_id: String,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedEvent {
    pub event_id: String,
    pub event_type: String,
    pub event_stream_id: String,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
    pub recorded_stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_position: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_position: Option<u64>,
    pub recorded_at: DateTime<Utc>,
}

impl EventData {
    pub fn new<E>(event: E) -> serde_json::Result<Self>
    where
        E: EventType + StreamEvent + Serialize,
    {
        Ok(Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: event.event_type().to_string(),
            stream_id: event.stream_id().to_string(),
            data: serde_json::to_string(&event)?,
            metadata: None,
        })
    }

    pub fn with_metadata<E, M>(event: E, metadata: Option<M>) -> serde_json::Result<Self>
    where
        E: EventType + StreamEvent + Serialize,
        M: Serialize,
    {
        Ok(Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: event.event_type().to_string(),
            stream_id: event.stream_id().to_string(),
            data: serde_json::to_string(&event)?,
            metadata: metadata
                .map(|value| serde_json::to_string(&value))
                .transpose()?,
        })
    }

    pub fn record(
        self,
        recorded_stream_id: impl Into<String>,
        stream_position: Option<u64>,
        log_position: Option<u64>,
        recorded_at: DateTime<Utc>,
    ) -> RecordedEvent {
        RecordedEvent {
            event_id: self.event_id,
            event_type: self.event_type,
            event_stream_id: self.stream_id,
            data: self.data,
            metadata: self.metadata,
            recorded_stream_id: recorded_stream_id.into(),
            stream_position,
            log_position,
            recorded_at,
        }
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }

    pub fn decode_data<E>(&self) -> serde_json::Result<E>
    where
        E: DeserializeOwned,
    {
        serde_json::from_str(&self.data)
    }

    pub fn decode_metadata<M>(&self) -> serde_json::Result<Option<M>>
    where
        M: DeserializeOwned,
    {
        self.metadata
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
    }

    pub fn decode(payload: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice::<Self>(payload)
    }
}

impl RecordedEvent {
    pub fn stream_id(&self) -> &str {
        &self.event_stream_id
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }

    pub fn decode_data<E>(&self) -> serde_json::Result<E>
    where
        E: DeserializeOwned,
    {
        serde_json::from_str(&self.data)
    }

    pub fn decode_metadata<M>(&self) -> serde_json::Result<Option<M>>
    where
        M: DeserializeOwned,
    {
        self.metadata
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
    }

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
        })
        .unwrap();

        assert_eq!(event.stream_id(), "alpha");
        assert_eq!(event.event_type, "TestEvent");
        assert_eq!(
            event.subject_with_prefix("events.test."),
            "events.test.alpha"
        );
        assert_eq!(event.decode_data::<TestEvent>().unwrap().value, "beta");
    }

    #[test]
    fn recorded_event_preserves_store_context() {
        let event = EventData::new(TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        })
        .unwrap();

        let recorded = event.record(
            "stream-alpha",
            Some(2),
            Some(10),
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.recorded_stream_id, "stream-alpha");
        assert_eq!(recorded.stream_position, Some(2));
        assert_eq!(recorded.log_position, Some(10));
        assert_eq!(
            recorded.subject_with_prefix("events.test."),
            "events.test.alpha"
        );
    }

    #[test]
    fn event_data_and_recorded_event_decode_payloads() {
        let event = EventData::new(TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        })
        .unwrap();
        let event_payload = serde_json::to_vec(&event).unwrap();
        let decoded_event = EventData::decode(&event_payload).unwrap();
        assert_eq!(decoded_event, event);
        assert_eq!(
            decoded_event.decode_data::<TestEvent>().unwrap().id,
            "alpha"
        );

        let recorded = event.record(
            "stream-alpha",
            None,
            Some(42),
            DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
        );
        let recorded_payload = serde_json::to_vec(&recorded).unwrap();
        let decoded_recorded = RecordedEvent::decode(&recorded_payload).unwrap();
        assert_eq!(decoded_recorded, recorded);
        assert_eq!(
            decoded_recorded.decode_data::<TestEvent>().unwrap().id,
            "alpha"
        );
    }
}
