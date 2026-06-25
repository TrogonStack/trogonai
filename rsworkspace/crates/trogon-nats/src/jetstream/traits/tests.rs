use super::PurgeOutcome;
use async_nats::jetstream::stream;
use serde_json::json;

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
