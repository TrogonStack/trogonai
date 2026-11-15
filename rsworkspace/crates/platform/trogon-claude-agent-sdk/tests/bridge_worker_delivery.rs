//! The delivery acknowledgement queue rules the recorded goldens reach only at small sizes: the batch and queue
//! limits, what leaves alone and what leaves together, and the retry delay (bridge.mjs offsets 1042101 and 1138528).

use std::time::Duration;

use serde_json::json;
use trogon_claude_agent_sdk::{
    DeliveryAck, DeliveryNext, DeliveryQueue, DeliveryStatus, MAX_BATCH_ACKS, MAX_QUEUE_ACKS, delivery_body,
    event_retry_delay,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn ack(id: &str, status: DeliveryStatus) -> DeliveryAck {
    DeliveryAck {
        event_id: id.to_owned(),
        status,
    }
}

fn received(id: &str) -> DeliveryAck {
    ack(id, DeliveryStatus::Received)
}

fn sends(next: DeliveryNext) -> Result<Vec<DeliveryAck>, String> {
    match next {
        DeliveryNext::Send(batch) => Ok(batch),
        other => Err(format!("expected a send, got {other:?}")),
    }
}

fn ids(batch: &[DeliveryAck]) -> Vec<&str> {
    batch.iter().map(|ack| ack.event_id.as_str()).collect()
}

#[test]
fn the_first_ack_leaves_alone_and_those_queued_meanwhile_leave_together() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("a")]);
    queue.enqueue(vec![received("b")]);
    queue.enqueue(vec![received("c")]);
    assert_eq!(ids(&sends(queue.poll())?), ["a"]);
    queue.done();
    assert_eq!(ids(&sends(queue.poll())?), ["b", "c"]);
    queue.done();
    assert_eq!(queue.poll(), DeliveryNext::Idle);
    Ok(())
}

#[test]
fn a_call_that_enqueues_several_acks_sends_them_together() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("a"), received("b")]);
    queue.enqueue(vec![received("c")]);
    assert_eq!(ids(&sends(queue.poll())?), ["a", "b"]);
    queue.done();
    assert_eq!(ids(&sends(queue.poll())?), ["c"]);
    Ok(())
}

#[test]
fn an_idle_queue_sends_the_next_ack_alone_again() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("a")]);
    sends(queue.poll())?;
    queue.done();
    assert_eq!(queue.poll(), DeliveryNext::Idle);
    queue.enqueue(vec![received("b")]);
    queue.enqueue(vec![received("c")]);
    assert_eq!(ids(&sends(queue.poll())?), ["b"]);
    Ok(())
}

#[test]
fn a_batch_holds_at_most_sixty_four_acks() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("first")]);
    sends(queue.poll())?;
    queue.done();
    queue.enqueue((0..MAX_QUEUE_ACKS).map(|n| received(&n.to_string())).collect());
    assert_eq!(sends(queue.poll())?.len(), MAX_BATCH_ACKS);
    queue.done();
    assert_eq!(queue.poll(), DeliveryNext::Idle);
    Ok(())
}

#[test]
fn the_queue_has_room_for_sixty_four_waiting_acks() -> TestResult {
    let mut queue = DeliveryQueue::default();
    assert!(queue.has_room_for(MAX_QUEUE_ACKS));
    assert!(!queue.has_room_for(MAX_QUEUE_ACKS + 1));
    queue.enqueue(vec![received("a")]);
    assert!(queue.has_room_for(MAX_QUEUE_ACKS - 1));
    assert!(!queue.has_room_for(MAX_QUEUE_ACKS));
    sends(queue.poll())?;
    assert!(queue.has_room_for(MAX_QUEUE_ACKS));
    Ok(())
}

#[test]
fn a_failed_batch_goes_back_to_the_front_and_waits_the_retry_delay() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("a")]);
    queue.enqueue(vec![received("b")]);
    sends(queue.poll())?;
    queue.failed(0.0, None);
    assert_eq!(queue.poll(), DeliveryNext::Pause(Duration::from_millis(500)));
    assert_eq!(ids(&sends(queue.poll())?), ["a", "b"]);
    queue.failed(0.5, None);
    assert_eq!(queue.poll(), DeliveryNext::Pause(Duration::from_millis(1250)));
    Ok(())
}

#[test]
fn a_taken_batch_resets_the_retry_delay() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("a")]);
    sends(queue.poll())?;
    queue.failed(0.0, None);
    queue.poll();
    sends(queue.poll())?;
    queue.done();
    queue.enqueue(vec![received("b")]);
    sends(queue.poll())?;
    queue.failed(0.0, None);
    assert_eq!(queue.poll(), DeliveryNext::Pause(Duration::from_millis(500)));
    Ok(())
}

#[test]
fn a_retry_after_sets_the_wait_between_the_base_and_the_ceiling() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("a")]);
    sends(queue.poll())?;
    queue.failed(0.0, Some(2000.0));
    assert_eq!(queue.poll(), DeliveryNext::Pause(Duration::from_millis(2000)));
    sends(queue.poll())?;
    queue.failed(0.0, Some(1.0));
    assert_eq!(queue.poll(), DeliveryNext::Pause(event_retry_delay(2, Some(1.0), 0.0)));
    Ok(())
}

#[test]
fn a_dropped_batch_is_counted_and_not_resent() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("a")]);
    sends(queue.poll())?;
    queue.dropped();
    assert_eq!(queue.dropped_batches(), 1);
    assert_eq!(queue.poll(), DeliveryNext::Idle);
    Ok(())
}

#[test]
fn closing_drops_what_is_queued_and_refuses_more() -> TestResult {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(vec![received("a"), received("b")]);
    queue.enqueue(vec![received("c")]);
    sends(queue.poll())?;
    queue.close();
    queue.failed(0.0, None);
    assert!(queue.is_closed());
    assert_eq!(queue.pending_count(), 0);
    queue.enqueue(vec![received("d")]);
    assert_eq!(queue.poll(), DeliveryNext::Idle);
    assert!(queue.is_idle());
    assert!(!queue.has_room_for(1));
    Ok(())
}

#[test]
fn the_body_carries_the_epoch_and_one_update_per_ack() {
    let body = delivery_body(
        7,
        &[
            received("a"),
            ack("b", DeliveryStatus::Processing),
            ack("c", DeliveryStatus::Processed),
        ],
    );
    assert_eq!(
        body,
        json!({"worker_epoch": 7, "updates": [
            {"event_id": "a", "status": "received"},
            {"event_id": "b", "status": "processing"},
            {"event_id": "c", "status": "processed"},
        ]})
    );
}
