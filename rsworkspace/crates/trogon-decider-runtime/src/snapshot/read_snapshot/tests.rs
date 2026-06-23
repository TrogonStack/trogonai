use super::*;
    use crate::StreamPosition;

    #[test]
    fn response_returns_loaded_snapshot() {
        let position = StreamPosition::try_new(7).unwrap();
        let snapshot = Snapshot::new(position, "payload");
        let response = ReadSnapshotResponse {
            snapshot: Some(snapshot.clone()),
        };

        assert_eq!(response.into_snapshot(), Some(snapshot));
    }
