//! The patch queue rules the bridge worker client reaches only through the close path, which the bridge entrypoint
//! drives from a handle the goldens do not close mid-flight (bridge.mjs offsets 1005399 to 1011500).

use std::time::Duration;

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    Next, Patch, PatchQueue, merge_patches, requeue_after_refusal, retry_delay, subtract_patches, without_status,
};

fn patch(value: Value) -> Patch {
    value.as_object().cloned().unwrap_or_default()
}

fn status() -> Patch {
    patch(
        json!({"worker_status": "running", "requires_action_details": null, "internal_metadata": {"reply_degraded": false}}),
    )
}

fn metadata(key: &str) -> Patch {
    patch(json!({"external_metadata": {key: 1}}))
}

fn sends(next: Next) -> Patch {
    match next {
        Next::Send(patch) => patch,
        other => panic!("expected a send, got {other:?}"),
    }
}

#[test]
fn merging_joins_the_metadata_groups_one_level_deep() {
    let merged = merge_patches(
        &patch(json!({"external_metadata": {"a": 1, "b": {"x": 1}}, "worker_status": "idle"})),
        &patch(json!({"external_metadata": {"b": {"y": 2}}, "worker_status": "running"})),
    );
    assert_eq!(
        Value::Object(merged),
        json!({"external_metadata": {"a": 1, "b": {"y": 2}}, "worker_status": "running"})
    );
}

#[test]
fn subtracting_removes_the_keys_the_other_patch_carries() {
    let left = subtract_patches(
        &patch(json!({"external_metadata": {"a": 1, "b": 2}, "internal_metadata": {"c": 3}})),
        &patch(json!({"external_metadata": {"a": 9}, "internal_metadata": {"c": 9}})),
    );
    assert_eq!(left.map(Value::Object), Some(json!({"external_metadata": {"b": 2}})));
    assert_eq!(
        subtract_patches(&metadata("a"), &metadata("a")),
        None,
        "nothing is left once every key is carried"
    );
}

#[test]
fn a_status_keeps_only_the_riders_that_outlive_it() {
    assert_eq!(
        without_status(&status()),
        None,
        "a rider that is set goes with its status"
    );
    let owed = patch(json!({
        "worker_status": "idle",
        "internal_metadata": {"reply_degraded": true, "card_turn_stamp": null, "last_served_model": "m"}
    }));
    assert_eq!(
        without_status(&owed).map(Value::Object),
        Some(json!({"internal_metadata": {"card_turn_stamp": null, "last_served_model": "m"}}))
    );
    assert_eq!(without_status(&metadata("a")), Some(metadata("a")));
}

#[test]
fn the_retry_delay_doubles_to_a_ceiling_and_adds_the_jitter() {
    let millis = |attempt, jitter| retry_delay(attempt, jitter).as_millis();
    assert_eq!(
        [1, 2, 3, 4, 7, 8, 30].map(|attempt| millis(attempt, 0.0)),
        [500, 1000, 2000, 4000, 30000, 30000, 30000]
    );
    assert_eq!(millis(1, 0.5), 750);
}

#[test]
fn closing_to_new_patches_drops_a_waiting_status_and_keeps_what_outlives_it() {
    let mut queue = PatchQueue::new();
    queue.enqueue(metadata("a"));
    queue.enqueue(status());
    queue.enqueue(patch(json!({"internal_metadata": {"card_turn_stamp": null}})));
    queue.close_to_new_patches();
    assert!(queue.is_closed_to_new_patches());
    assert!(queue.has_dropped_something());
    queue.enqueue(metadata("late"));
    assert_eq!(
        Value::Object(sends(queue.poll(0.0))),
        json!({"external_metadata": {"a": 1}}),
        "the patch that was already sending goes out unchanged"
    );
    queue.sent(true);
    assert_eq!(
        Value::Object(sends(queue.poll(0.0))),
        json!({"internal_metadata": {"card_turn_stamp": null}}),
        "the waiting status is cut down to what outlives it"
    );
    assert!(queue.owes_metadata());
    queue.sent(true);
    assert_eq!(queue.poll(0.0), Next::Idle);
    assert!(!queue.owes_metadata());
}

#[test]
fn a_status_that_fails_after_the_queue_closed_is_handed_back_without_it() {
    let mut queue = PatchQueue::new();
    queue.enqueue(patch(json!({
        "worker_status": "running",
        "internal_metadata": {"reply_degraded": false, "last_served_model": "m"}
    })));
    assert!(matches!(queue.poll(0.0), Next::Send(_)));
    queue.close_to_new_patches();
    queue.sent(false);
    assert_eq!(
        Value::Object(sends(queue.poll(0.0))),
        json!({"internal_metadata": {"last_served_model": "m"}})
    );
    assert!(queue.has_dropped_something());
}

#[test]
fn closing_drops_everything_that_has_not_gone_out() {
    let mut queue = PatchQueue::new();
    queue.enqueue(metadata("a"));
    queue.enqueue(metadata("b"));
    queue.close();
    assert!(queue.is_closed());
    assert!(
        matches!(queue.poll(0.0), Next::Send(_)),
        "the claimed patch still makes its first attempt"
    );
    queue.sent(false);
    assert_eq!(queue.poll(0.0), Next::Idle);
    queue.enqueue(metadata("c"));
    assert!(queue.has_dropped_something());
    assert_eq!(queue.poll(0.0), Next::Idle);
}

#[test]
fn a_failed_patch_waits_before_it_is_sent_again() {
    let mut queue = PatchQueue::new();
    queue.enqueue(metadata("a"));
    assert!(matches!(queue.poll(0.0), Next::Send(_)));
    queue.sent(false);
    assert_eq!(queue.poll(0.0), Next::Pause(Duration::from_millis(500)));
    queue.enqueue(metadata("b"));
    assert_eq!(
        Value::Object(sends(queue.poll(0.0))),
        json!({"external_metadata": {"a": 1, "b": 1}}),
        "what was queued during the wait joins the retry"
    );
    queue.sent(false);
    assert_eq!(queue.poll(0.0), Next::Pause(Duration::from_millis(1000)));
}

#[test]
fn a_refusal_gives_back_the_metadata_and_the_bare_status() {
    let sent = patch(json!({
        "worker_status": "idle",
        "requires_action_details": null,
        "internal_metadata": {"reply_degraded": false},
        "external_metadata": {"plan_doc": "x", "model": "m"}
    }));
    let back = requeue_after_refusal(&sent);
    assert_eq!(
        back.metadata.map(Value::Object),
        Some(json!({"external_metadata": {"plan_doc": "x"}}))
    );
    assert_eq!(
        back.status.map(Value::Object),
        Some(
            json!({"worker_status": "idle", "requires_action_details": null, "internal_metadata": {"reply_degraded": false}})
        )
    );
    assert_eq!(
        requeue_after_refusal(&status()).status,
        None,
        "a bare status is not sent again"
    );
    assert_eq!(requeue_after_refusal(&metadata("a")).metadata, None);
}

#[test]
fn a_requeued_status_gives_way_to_one_that_is_waiting() {
    let mut queue = PatchQueue::new();
    queue.enqueue(metadata("a"));
    queue.enqueue(status());
    queue.requeue_status(patch(json!({"worker_status": "idle"})));
    assert!(matches!(queue.poll(0.0), Next::Send(_)));
    queue.sent(true);
    assert_eq!(
        sends(queue.poll(0.0)).get("worker_status"),
        Some(&json!("running")),
        "the status that was already waiting wins"
    );
}

#[test]
fn requeued_metadata_goes_out_after_the_waiting_patch_and_skips_what_it_carries() {
    let mut queue = PatchQueue::new();
    queue.enqueue(metadata("x"));
    queue.requeue_metadata(patch(json!({"external_metadata": {"plan_doc": 1, "x": 2}})));
    queue.enqueue(metadata("x"));
    assert!(matches!(queue.poll(0.0), Next::Send(_)));
    queue.sent(true);
    assert_eq!(
        Value::Object(sends(queue.poll(0.0))),
        json!({"external_metadata": {"x": 1}})
    );
    queue.sent(true);
    assert_eq!(
        Value::Object(sends(queue.poll(0.0))),
        json!({"external_metadata": {"plan_doc": 1}}),
        "a key a later patch carries is not sent from the requeue"
    );
}

#[test]
fn requeued_metadata_sends_the_switched_off_servers_alone() {
    let mut queue = PatchQueue::new();
    queue.enqueue(metadata("seed"));
    queue.requeue_metadata(patch(
        json!({"internal_metadata": {"disabled_mcp_servers": ["a"], "cost_state": 1}}),
    ));
    assert!(matches!(queue.poll(0.0), Next::Send(_)));
    queue.sent(true);
    assert_eq!(
        Value::Object(sends(queue.poll(0.0))),
        json!({"internal_metadata": {"cost_state": 1}})
    );
    queue.sent(true);
    assert_eq!(
        Value::Object(sends(queue.poll(0.0))),
        json!({"internal_metadata": {"disabled_mcp_servers": ["a"]}})
    );
}
