    use super::*;
    use serde::{Deserialize, Serialize};
    use trogon_decider_runtime::StreamPosition;
    use trogon_decider_runtime::snapshot::{
        InvalidSnapshotTypeName, SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
        SnapshotTypeName,
    };

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct TestPayload {
        id: String,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("missing snapshot type")]
    struct TestSnapshotTypeError;

    #[derive(Debug)]
    struct UnavailableSnapshotTypePayload;

    impl SnapshotType for TestPayload {
        type Error = InvalidSnapshotTypeName;

        fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
            SnapshotTypeName::new("test.snapshot.v2")
        }
    }

    impl SnapshotType for UnavailableSnapshotTypePayload {
        type Error = TestSnapshotTypeError;

        fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
            Err(TestSnapshotTypeError)
        }
    }

    impl SnapshotPayloadEncode for TestPayload {
        type Error = serde_json::Error;

        fn encode(&self) -> Result<Vec<u8>, Self::Error> {
            serde_json::to_vec(self)
        }
    }

    impl SnapshotPayloadDecode for TestPayload {
        type Error = serde_json::Error;

        fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
            serde_json::from_slice(payload.payload)
        }
    }

    impl SnapshotPayloadDecode for UnavailableSnapshotTypePayload {
        type Error = serde_json::Error;

        fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
            serde_json::from_slice::<serde_json::Value>(payload.payload).map(|_| Self)
        }
    }

    #[test]
    fn nats_snapshot_config_exposes_checkpoint_name() {
        let config = NatsSnapshotConfig::with_checkpoint_name("last_event_sequence");

        assert_eq!(config.checkpoint_name(), Some("last_event_sequence"));
    }

    #[test]
    fn snapshot_key_uses_nats_snapshot_namespace() {
        assert_eq!(
            snapshot_key::<TestPayload>("backup").unwrap(),
            "snapshots.data.test.snapshot.v2.backup"
        );
    }

    #[test]
    fn snapshot_key_preserves_snapshot_type_error() {
        assert_eq!(
            snapshot_key::<UnavailableSnapshotTypePayload>("backup")
                .unwrap_err()
                .to_string(),
            "Snapshot codec error: failed to resolve stream snapshot type: missing snapshot type"
        );
    }

    #[test]
    fn unavailable_snapshot_type_payload_decode_accepts_json() {
        assert!(UnavailableSnapshotTypePayload::decode(SnapshotPayloadData::new(br#"{"id":"backup"}"#)).is_ok());
    }

    #[test]
    fn checkpoint_key_uses_nats_specific_format() {
        let config = NatsSnapshotConfig::with_checkpoint_name("last_event_sequence");

        assert_eq!(
            checkpoint_key::<TestPayload>(&config).unwrap(),
            "snapshots.checkpoint.test.snapshot.v2.last_event_sequence"
        );
    }

    #[test]
    fn checkpoint_key_requires_configured_name() {
        let config = NatsSnapshotConfig::without_checkpoint();

        assert_eq!(
            checkpoint_key::<TestPayload>(&config).unwrap_err().to_string(),
            "Missing checkpoint name for snapshot type: test.snapshot.v2"
        );
    }

    #[test]
    fn snapshot_constructors_keep_position_and_payload() {
        let snapshot = Snapshot::new(
            position(9),
            TestPayload {
                id: "backup".to_string(),
            },
        );

        assert_eq!(snapshot.position, position(9));
        assert_eq!(snapshot.payload.id, "backup");
    }

    #[test]
    fn snapshot_change_builders_keep_snapshot_identity() {
        let upsert = SnapshotChange::upsert(
            "backup",
            Snapshot::new(
                position(3),
                TestPayload {
                    id: "backup".to_string(),
                },
            ),
        );
        let delete = SnapshotChange::<TestPayload>::delete("backup");

        assert_eq!(
            upsert,
            SnapshotChange::Upsert {
                snapshot_id: "backup".to_string(),
                snapshot: Box::new(Snapshot::new(
                    position(3),
                    TestPayload {
                        id: "backup".to_string(),
                    },
                )),
            }
        );
        assert_eq!(
            delete,
            SnapshotChange::Delete {
                snapshot_id: "backup".to_string(),
            }
        );
    }

    #[test]
    fn stored_snapshot_round_trips_with_nested_payload() {
        let snapshot = Snapshot::new(
            position(7),
            TestPayload {
                id: "backup".to_string(),
            },
        );

        let json = String::from_utf8(snapshot_value(&snapshot).unwrap()).unwrap();
        let decoded = snapshot_from_value::<TestPayload>(json.as_bytes()).unwrap();

        assert!(json.contains("\"type\":\"test.snapshot.v2\""));
        assert!(json.contains("\"position\":7"));
        assert!(json.contains("\"payload\""));
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn stored_snapshot_rejects_unexpected_snapshot_type() {
        let encoded = EncodedSnapshot::new("other.snapshot.v2", position(7), br#"{"id":"backup"}"#.to_vec())
            .into_bytes()
            .unwrap();

        assert_eq!(
            snapshot_from_value::<TestPayload>(&encoded).unwrap_err().to_string(),
            "Snapshot codec error: unexpected stream snapshot type: expected test.snapshot.v2, got other.snapshot.v2"
        );
    }

    #[test]
    fn stored_snapshot_preserves_payload_decode_error() {
        let encoded = EncodedSnapshot::new("test.snapshot.v2", position(7), b"not-json".to_vec())
            .into_bytes()
            .unwrap();

        assert_eq!(
            snapshot_from_value::<TestPayload>(&encoded).unwrap_err().to_string(),
            "Snapshot codec error: failed to decode stream snapshot payload: expected ident at line 1 column 2"
        );
    }

    #[test]
    fn stored_snapshot_preserves_snapshot_type_error() {
        let encoded = EncodedSnapshot::new("test.snapshot.v2", position(7), br#"{"id":"backup"}"#.to_vec())
            .into_bytes()
            .unwrap();

        assert_eq!(
            snapshot_from_value::<UnavailableSnapshotTypePayload>(&encoded)
                .unwrap_err()
                .to_string(),
            "Snapshot codec error: failed to resolve stream snapshot type: missing snapshot type"
        );
    }

    #[test]
    fn stored_snapshot_position_rejects_unexpected_snapshot_type() {
        let encoded = EncodedSnapshot::new("other.snapshot.v2", position(7), br#"{"id":"backup"}"#.to_vec())
            .into_bytes()
            .unwrap();

        assert_eq!(
            snapshot_position_from_value::<TestPayload, serde_json::Error>(&encoded)
                .unwrap_err()
                .to_string(),
            "Snapshot codec error: unexpected stream snapshot type: expected test.snapshot.v2, got other.snapshot.v2"
        );
    }

    #[test]
    fn snapshot_id_from_snapshot_key_uses_configured_prefix() {
        assert_eq!(
            snapshot_id_from_snapshot_key::<TestPayload, serde_json::Error>("snapshots.data.test.snapshot.v2.backup")
                .unwrap(),
            Some("backup".to_string())
        );
        assert_eq!(
            snapshot_id_from_snapshot_key::<TestPayload, serde_json::Error>("snapshots.data.test.snapshot.v1.backup")
                .unwrap(),
            None
        );
    }

    #[test]
    fn snapshot_id_from_snapshot_key_rejects_empty_suffix() {
        assert_eq!(
            snapshot_id_from_snapshot_key::<TestPayload, serde_json::Error>("snapshots.data.test.snapshot.v2.")
                .unwrap_err()
                .to_string(),
            "Invalid stream snapshot key: snapshots.data.test.snapshot.v2."
        );
    }

    mod kv_operations;

