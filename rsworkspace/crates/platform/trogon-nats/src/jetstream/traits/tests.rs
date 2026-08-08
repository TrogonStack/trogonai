use super::{ProvisionedStreamField, PurgeOutcome, reconciled_stream_config};
use async_nats::jetstream::stream;
use serde_json::json;
use std::time::Duration;

fn purge_response(success: bool) -> stream::PurgeResponse {
    serde_json::from_value(json!({ "success": success, "purged": 0_u64 })).unwrap()
}

#[test]
fn unit_purge_outcome_is_always_successful() {
    assert!(().is_success());
}

#[test]
fn purge_response_outcome_reflects_success_field() {
    assert!(purge_response(true).is_success());
    assert!(!purge_response(false).is_success());
}

fn operator_managed() -> stream::Config {
    stream::Config {
        name: "OPERATED".to_owned(),
        subjects: vec!["stale.>".to_owned()],
        duplicate_window: Duration::from_secs(120),
        num_replicas: 3,
        storage: stream::StorageType::Memory,
        max_bytes: 1_024,
        ..Default::default()
    }
}

#[test]
fn reconciling_applies_only_the_named_fields() {
    let declared = stream::Config {
        name: "OPERATED".to_owned(),
        subjects: vec!["fresh.>".to_owned()],
        duplicate_window: Duration::from_secs(300),
        max_age: Duration::from_secs(3_600),
        ..Default::default()
    };

    let reconciled = reconciled_stream_config(
        &operator_managed(),
        &declared,
        &[ProvisionedStreamField::Subjects, ProvisionedStreamField::MaxAge],
    );

    assert_eq!(reconciled.subjects, vec!["fresh.>"]);
    assert_eq!(reconciled.max_age, Duration::from_secs(3_600));
    // Unnamed, so the declared value never reaches the server, defaults least
    // of all: this is the roll-back a whole-config update would have caused.
    assert_eq!(reconciled.duplicate_window, Duration::from_secs(120));
    assert_eq!(reconciled.num_replicas, 3);
    assert_eq!(reconciled.storage, stream::StorageType::Memory);
    assert_eq!(reconciled.max_bytes, 1_024);
}

#[test]
fn reconciling_nothing_leaves_the_live_config_alone() {
    let live = operator_managed();
    assert_eq!(reconciled_stream_config(&live, &stream::Config::default(), &[]), live);
}
