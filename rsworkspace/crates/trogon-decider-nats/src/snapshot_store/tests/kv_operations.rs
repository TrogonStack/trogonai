        use super::*;
        use bytes::Bytes;
        use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

        fn snapshot_payload(id: &str) -> TestPayload {
            TestPayload { id: id.to_string() }
        }

        fn encoded_snapshot_at(position_value: u64, id: &str) -> Vec<u8> {
            snapshot_value(&Snapshot::new(position(position_value), snapshot_payload(id))).unwrap()
        }

        fn encoded_snapshot_with_type(snapshot_type: &str, position_value: u64, id: &str) -> Vec<u8> {
            let payload = snapshot_payload(id).encode().unwrap();
            EncodedSnapshot::new(snapshot_type, position(position_value), payload)
                .into_bytes()
                .unwrap()
        }

        #[tokio::test]
        async fn write_snapshot_creates_entry_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();

            write_snapshot(&bucket, "alpha", Snapshot::new(position(5), snapshot_payload("alpha")))
                .await
                .expect("write should succeed");

            let calls = bucket.create_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "snapshots.data.test.snapshot.v2.alpha");
        }

        #[tokio::test]
        async fn write_snapshot_skips_when_existing_position_is_at_or_above_new() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(10, "alpha")), 3, kv::Operation::Put);

            write_snapshot(&bucket, "alpha", Snapshot::new(position(5), snapshot_payload("alpha")))
                .await
                .expect("skip should be silent");

            assert!(bucket.update_calls().is_empty());
            assert!(bucket.create_calls().is_empty());
        }

        #[tokio::test]
        async fn write_snapshot_updates_when_existing_position_is_lower() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(3, "alpha")), 7, kv::Operation::Put);

            write_snapshot(&bucket, "alpha", Snapshot::new(position(9), snapshot_payload("alpha")))
                .await
                .expect("update should succeed");

            let updates = bucket.update_calls();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].0, "snapshots.data.test.snapshot.v2.alpha");
            assert_eq!(updates[0].2, 7);
        }

        #[tokio::test]
        async fn write_snapshot_retries_on_wrong_last_revision() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(1, "alpha")), 4, kv::Operation::Put);
            bucket.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(2, "alpha")), 5, kv::Operation::Put);
            bucket.enqueue_update_result(Ok(6));

            write_snapshot(&bucket, "alpha", Snapshot::new(position(10), snapshot_payload("alpha")))
                .await
                .expect("write should succeed after retry");

            assert_eq!(bucket.update_calls().len(), 2);
            assert_eq!(bucket.entry_calls().len(), 2);
        }

        #[tokio::test]
        async fn write_snapshot_creates_when_entry_is_tombstone() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::new(), 4, kv::Operation::Delete);
            bucket.enqueue_create_result(Ok(5));

            write_snapshot(&bucket, "alpha", Snapshot::new(position(2), snapshot_payload("alpha")))
                .await
                .expect("write should succeed");

            assert_eq!(bucket.create_calls().len(), 1);
            assert!(bucket.update_calls().is_empty());
        }

        #[tokio::test]
        async fn write_snapshot_retries_when_create_already_exists() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            bucket.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(1, "alpha")), 2, kv::Operation::Put);
            bucket.enqueue_update_result(Ok(3));

            write_snapshot(&bucket, "alpha", Snapshot::new(position(5), snapshot_payload("alpha")))
                .await
                .expect("write should succeed after race");

            assert_eq!(bucket.create_calls().len(), 1);
            assert_eq!(bucket.update_calls().len(), 1);
        }

        #[tokio::test]
        async fn persist_snapshot_change_delete_is_idempotent_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();

            persist_snapshot_change::<TestPayload, _>(&bucket, SnapshotChange::delete("alpha"))
                .await
                .expect("delete should be idempotent");

            assert!(bucket.delete_calls().is_empty());
        }

        #[tokio::test]
        async fn persist_snapshot_change_delete_is_idempotent_on_tombstone() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::new(), 3, kv::Operation::Delete);

            persist_snapshot_change::<TestPayload, _>(&bucket, SnapshotChange::delete("alpha"))
                .await
                .expect("delete should be idempotent");

            assert!(bucket.delete_calls().is_empty());
        }

        #[tokio::test]
        async fn persist_snapshot_change_delete_retries_on_wrong_last_revision() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(1, "alpha")), 3, kv::Operation::Put);
            bucket.enqueue_delete_result(Err(kv::DeleteErrorKind::WrongLastRevision));
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(2, "alpha")), 5, kv::Operation::Put);
            bucket.enqueue_delete_result(Ok(()));

            persist_snapshot_change::<TestPayload, _>(&bucket, SnapshotChange::delete("alpha"))
                .await
                .expect("delete should succeed after retry");

            assert_eq!(bucket.delete_calls().len(), 2);
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_creates_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1)
                .await
                .expect("create should succeed");

            let calls = bucket.create_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "snapshots.checkpoint.test.snapshot.v2.checkpoint");
            assert_eq!(calls[0].1, Bytes::from_static(b"1"));
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_updates_when_one_behind() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"4"), 9, kv::Operation::Put);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 5)
                .await
                .expect("update should succeed");

            let updates = bucket.update_calls();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].2, 9);
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_is_noop_when_not_one_behind() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"3"), 7, kv::Operation::Put);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 7)
                .await
                .expect("noop should not error");

            assert!(bucket.update_calls().is_empty());
            assert!(bucket.create_calls().is_empty());
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_tolerates_wrong_last_revision() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"1"), 4, kv::Operation::Put);
            bucket.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 2)
                .await
                .expect("conflict should be silent");

            assert_eq!(bucket.update_calls().len(), 1);
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_tolerates_already_exists() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            bucket.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1)
                .await
                .expect("race should be silent");

            assert_eq!(bucket.create_calls().len(), 1);
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_treats_tombstone_as_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::new(), 8, kv::Operation::Delete);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1)
                .await
                .expect("tombstone should create fresh checkpoint");

            assert_eq!(bucket.create_calls().len(), 1);
            assert!(bucket.update_calls().is_empty());
        }

        #[tokio::test]
        async fn read_checkpoint_returns_zero_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            let sequence = read_checkpoint::<TestPayload, _>(&bucket, &config).await.unwrap();

            assert_eq!(sequence, 0);
        }

        #[tokio::test]
        async fn read_checkpoint_decodes_stored_sequence() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"42"), 1, kv::Operation::Put);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            let sequence = read_checkpoint::<TestPayload, _>(&bucket, &config).await.unwrap();

            assert_eq!(sequence, 42);
        }

        #[tokio::test]
        async fn write_checkpoint_creates_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            write_checkpoint::<TestPayload, _>(&bucket, &config, 11)
                .await
                .expect("write should succeed");

            let calls = bucket.create_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].1, Bytes::from_static(b"11"));
        }

        #[tokio::test]
        async fn write_checkpoint_updates_when_present() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"4"), 3, kv::Operation::Put);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            write_checkpoint::<TestPayload, _>(&bucket, &config, 5)
                .await
                .expect("write should succeed");

            let updates = bucket.update_calls();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].1, Bytes::from_static(b"5"));
            assert_eq!(updates[0].2, 3);
        }

        #[tokio::test]
        async fn read_snapshot_entries_lists_keys_and_decodes_values() {
            let bucket = MockJetStreamKvStore::new();
            bucket.set_keys_result(Ok(vec![
                "snapshots.data.test.snapshot.v2.alpha".to_string(),
                "snapshots.data.test.snapshot.v1.legacy".to_string(),
                "snapshots.data.test.snapshot.v2.beta".to_string(),
            ]));
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(3, "alpha")));
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(7, "beta")));

            let entries = list_snapshots::<TestPayload, _>(&bucket).await.unwrap();

            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].position, position(3));
            assert_eq!(entries[1].position, position(7));
            assert_eq!(bucket.keys_calls(), 1);
        }

        #[tokio::test]
        async fn read_snapshot_entries_skips_missing_values() {
            let bucket = MockJetStreamKvStore::new();
            bucket.set_keys_result(Ok(vec![
                "snapshots.data.test.snapshot.v2.alpha".to_string(),
                "snapshots.data.test.snapshot.v2.beta".to_string(),
            ]));
            bucket.enqueue_get_none();
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(2, "beta")));

            let entries = list_snapshots::<TestPayload, _>(&bucket).await.unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].position, position(2));
        }

        #[tokio::test]
        async fn read_snapshot_entries_skips_colliding_snapshot_type_keys() {
            let bucket = MockJetStreamKvStore::new();
            bucket.set_keys_result(Ok(vec![
                "snapshots.data.test.snapshot.v2.alpha".to_string(),
                "snapshots.data.test.snapshot.v2.extra.beta".to_string(),
            ]));
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(3, "alpha")));
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_with_type(
                "test.snapshot.v2.extra",
                7,
                "beta",
            )));

            let entries = read_snapshot_entries::<TestPayload, _>(&bucket).await.unwrap();

            assert_eq!(
                entries,
                vec![(
                    "alpha".to_string(),
                    Snapshot::new(position(3), snapshot_payload("alpha"))
                )]
            );
        }

        #[tokio::test]
        async fn read_snapshot_returns_none_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_get_none();

            let snapshot = read_snapshot::<TestPayload, _>(&bucket, "alpha").await.unwrap();

            assert_eq!(snapshot, None);
            assert_eq!(bucket.get_calls(), vec!["snapshots.data.test.snapshot.v2.alpha"]);
        }

        #[tokio::test]
        async fn read_snapshot_decodes_stored_value() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(9, "alpha")));

            let snapshot = read_snapshot::<TestPayload, _>(&bucket, "alpha")
                .await
                .unwrap()
                .unwrap();

            assert_eq!(snapshot.position, position(9));
            assert_eq!(snapshot.payload, snapshot_payload("alpha"));
        }

        #[tokio::test]
        async fn read_snapshot_map_preserves_snapshot_ids() {
            let bucket = MockJetStreamKvStore::new();
            bucket.set_keys_result(Ok(vec![
                "snapshots.data.test.snapshot.v2.alpha".to_string(),
                "snapshots.data.test.snapshot.v2.beta".to_string(),
            ]));
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(3, "alpha")));
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(7, "beta")));

            let snapshots = read_snapshot_map::<TestPayload, _>(&bucket).await.unwrap();

            assert_eq!(snapshots["alpha"].position, position(3));
            assert_eq!(snapshots["beta"].position, position(7));
        }
    
