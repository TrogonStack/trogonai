use super::*;
use trogon_nats::test_support::JetStreamTestServer;

fn snapshot_payload(id: &str) -> TestPayload {
    TestPayload { id: id.to_string() }
}

async fn create_bucket(bucket: &str) -> (JetStreamTestServer, kv::Store) {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let store = js
        .create_key_value(kv::Config {
            bucket: bucket.to_string(),
            ..Default::default()
        })
        .await
        .expect("create snapshot kv bucket");

    (server, store)
}

#[tokio::test]
async fn write_snapshot_then_read_snapshot_round_trips_through_real_kv_bucket() {
    let (_server, bucket) = create_bucket("SNAPSHOT_ROUND_TRIP").await;

    write_snapshot(&bucket, "alpha", Snapshot::new(position(5), snapshot_payload("alpha")))
        .await
        .expect("write should succeed");

    let snapshot = read_snapshot::<TestPayload, _>(&bucket, "alpha")
        .await
        .expect("read should succeed")
        .expect("snapshot should be present");

    assert_eq!(snapshot.position, position(5));
    assert_eq!(snapshot.payload, snapshot_payload("alpha"));
}

#[tokio::test]
async fn write_snapshot_keeps_highest_position_against_real_kv_bucket() {
    let (_server, bucket) = create_bucket("SNAPSHOT_VERSIONING").await;

    write_snapshot(&bucket, "alpha", Snapshot::new(position(5), snapshot_payload("alpha")))
        .await
        .expect("initial write should succeed");
    write_snapshot(&bucket, "alpha", Snapshot::new(position(3), snapshot_payload("stale")))
        .await
        .expect("stale write should be silently skipped");

    let snapshot = read_snapshot::<TestPayload, _>(&bucket, "alpha")
        .await
        .expect("read should succeed")
        .expect("snapshot should be present");
    assert_eq!(snapshot.position, position(5));
    assert_eq!(snapshot.payload, snapshot_payload("alpha"));

    write_snapshot(&bucket, "alpha", Snapshot::new(position(9), snapshot_payload("newer")))
        .await
        .expect("newer write should overwrite");

    let snapshot = read_snapshot::<TestPayload, _>(&bucket, "alpha")
        .await
        .expect("read should succeed")
        .expect("snapshot should be present");
    assert_eq!(snapshot.position, position(9));
    assert_eq!(snapshot.payload, snapshot_payload("newer"));
}

#[tokio::test]
async fn maybe_advance_checkpoint_versioning_against_real_kv_bucket() {
    let (_server, bucket) = create_bucket("SNAPSHOT_CHECKPOINT").await;
    let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

    assert_eq!(read_checkpoint::<TestPayload, _>(&bucket, &config).await.unwrap(), 0);

    maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1)
        .await
        .expect("first advance should create the checkpoint");
    assert_eq!(read_checkpoint::<TestPayload, _>(&bucket, &config).await.unwrap(), 1);

    maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 3)
        .await
        .expect("a gap ahead of the current checkpoint should be a silent noop");
    assert_eq!(read_checkpoint::<TestPayload, _>(&bucket, &config).await.unwrap(), 1);

    maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 2)
        .await
        .expect("the next sequential advance should update the checkpoint");
    assert_eq!(read_checkpoint::<TestPayload, _>(&bucket, &config).await.unwrap(), 2);
}
