//! The client event queue rules the recorded goldens reach only at small sizes: batch limits, the byte limit that a
//! request with no answer cuts, the attempt a batch is labelled with, and the retry delay (bridge.mjs offsets 988814
//! to 1005143).

use std::time::Duration;

use serde_json::{Map, Value, json};
use trogon_claude_agent_sdk::{
    Attempt, Batch, ClientEvent, EventNext, EventQueue, MAX_BATCH_EVENTS, SHRINK_FROM_BYTES, event_retry_delay,
    refused_for_good, upload_body,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn event(text: &str) -> ClientEvent {
    let mut payload = Map::new();
    payload.insert("type".to_owned(), json!("assistant"));
    payload.insert("text".to_owned(), json!(text));
    ClientEvent {
        payload,
        historical: false,
        ephemeral: false,
    }
}

fn thinking() -> ClientEvent {
    ClientEvent {
        ephemeral: true,
        ..event("t")
    }
}

fn sends(next: EventNext) -> Result<Batch, String> {
    match next {
        EventNext::Send(batch) => Ok(batch),
        other => Err(format!("expected a send, got {other:?}")),
    }
}

fn texts(batch: &Batch) -> Vec<String> {
    batch
        .events
        .iter()
        .map(|event| event.payload["text"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn a_batch_holds_at_most_one_hundred_events() -> TestResult {
    let mut queue = EventQueue::default();
    queue.enqueue((0..MAX_BATCH_EVENTS + 5).map(|n| event(&n.to_string())).collect());
    let first = sends(queue.poll())?;
    assert_eq!(first.events.len(), MAX_BATCH_EVENTS);
    assert_eq!(first.attempt, Attempt::First);
    assert!(first.cuttable);
    queue.done();
    assert_eq!(texts(&sends(queue.poll())?).len(), 5);
    queue.done();
    assert_eq!(queue.poll(), EventNext::Idle);
    Ok(())
}

#[test]
fn a_single_event_is_not_cuttable() -> TestResult {
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event("a")]);
    assert!(!sends(queue.poll())?.cuttable);
    Ok(())
}

#[test]
fn a_failed_batch_goes_back_in_front_of_what_arrived_meanwhile() -> TestResult {
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event("a")]);
    assert_eq!(sends(queue.poll())?.attempt, Attempt::First);
    queue.enqueue(vec![event("b")]);
    queue.failed(0.0, None, false);
    assert_eq!(queue.poll(), EventNext::Pause(Duration::from_millis(500)));
    let retry = sends(queue.poll())?;
    assert_eq!(texts(&retry), ["a", "b"]);
    assert_eq!(retry.attempt, Attempt::ResendWithNew);
    assert_eq!(retry.retry_count, 1);
    Ok(())
}

#[test]
fn a_batch_made_only_of_sent_events_is_a_resend() -> TestResult {
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event("a")]);
    sends(queue.poll())?;
    queue.failed(0.0, None, false);
    queue.poll();
    assert_eq!(sends(queue.poll())?.attempt, Attempt::Resend);
    queue.done();
    queue.enqueue(vec![event("b")]);
    assert_eq!(sends(queue.poll())?.attempt, Attempt::First);
    Ok(())
}

#[test]
fn the_wait_doubles_to_a_ceiling_and_a_success_resets_it() -> TestResult {
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event("a")]);
    let mut waits = Vec::new();
    for _ in 0..8 {
        sends(queue.poll())?;
        queue.failed(0.0, None, false);
        let EventNext::Pause(wait) = queue.poll() else {
            return Err("expected a pause".into());
        };
        waits.push(wait.as_millis());
    }
    assert_eq!(waits, [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000]);
    sends(queue.poll())?;
    queue.done();
    queue.enqueue(vec![event("b")]);
    sends(queue.poll())?;
    queue.failed(0.0, None, false);
    assert_eq!(queue.poll(), EventNext::Pause(Duration::from_millis(500)));
    Ok(())
}

#[test]
fn retry_delay_follows_the_service_and_adds_the_jitter() {
    let ms = |failures, asked, jitter| event_retry_delay(failures, asked, jitter).as_millis();
    assert_eq!(ms(1, None, 0.5), 750);
    assert_eq!(ms(3, None, 0.0), 2000);
    assert_eq!(ms(1, Some(100.0), 0.0), 500);
    assert_eq!(ms(1, Some(2000.0), 0.0), 2000);
    assert_eq!(ms(1, Some(90_000.0), 0.0), 30_000);
    assert_eq!(ms(2, Some(f64::NAN), 0.0), 1000);
    assert_eq!(ms(2, Some(f64::INFINITY), 0.0), 1000);
}

#[test]
fn a_request_with_no_answer_cuts_the_next_batches_by_half() -> TestResult {
    let big = "x".repeat(SHRINK_FROM_BYTES / 2);
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event(&big), event(&big), event(&big)]);
    let first = sends(queue.poll())?;
    assert!(first.bytes >= SHRINK_FROM_BYTES);
    assert_eq!(first.events.len(), 3);
    queue.failed(0.0, None, true);
    queue.poll();
    let smaller = sends(queue.poll())?;
    assert_eq!(smaller.events.len(), 1);
    Ok(())
}

#[test]
fn an_answered_failure_does_not_cut_the_batch() -> TestResult {
    let big = "x".repeat(SHRINK_FROM_BYTES / 2);
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event(&big), event(&big)]);
    let first = sends(queue.poll())?;
    queue.failed(0.0, None, false);
    queue.poll();
    assert_eq!(sends(queue.poll())?.events.len(), first.events.len());
    Ok(())
}

#[test]
fn live_view_rows_can_be_let_go_while_the_rest_stay() -> TestResult {
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event("a"), thinking()]);
    sends(queue.poll())?;
    assert_eq!(queue.keep_only(|event| !event.ephemeral), 1);
    assert_eq!(queue.in_flight_events().len(), 1);
    assert_eq!(queue.unsent(), 1);
    queue.done();
    assert_eq!(queue.unsent(), 0);
    Ok(())
}

#[test]
fn closing_hands_back_what_was_not_sent() -> TestResult {
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event("a"), event("b")]);
    sends(queue.poll())?;
    queue.enqueue(vec![event("c")]);
    assert_eq!(queue.close(true), 3);
    assert!(queue.is_closed());
    assert_eq!(queue.poll(), EventNext::Idle);
    queue.enqueue(vec![event("d")]);
    assert_eq!(queue.handed_over_after_close(), 1);
    let kept: Vec<String> = queue
        .take_undelivered()
        .iter()
        .map(|event| event.payload["text"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(kept, ["a", "b", "c", "d"]);
    assert!(queue.take_undelivered().is_empty());
    Ok(())
}

#[test]
fn closing_without_retaining_drops_them() {
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event("a")]);
    assert_eq!(queue.close(false), 1);
    assert!(queue.take_undelivered().is_empty());
}

#[test]
fn the_queue_reports_how_much_room_is_left() {
    let mut queue = EventQueue::new(3);
    assert!(queue.has_room_for(3));
    queue.enqueue(vec![event("a"), event("b")]);
    assert!(queue.has_room_for(1));
    assert!(!queue.has_room_for(2));
    assert_eq!(queue.capacity(), 3);
}

#[test]
fn only_these_statuses_are_refused_for_good() {
    for status in [400, 413, 422] {
        assert!(refused_for_good(status), "{status}");
    }
    for status in [401, 403, 404, 409, 429, 500, 503] {
        assert!(!refused_for_good(status), "{status}");
    }
}

#[test]
fn the_body_carries_the_epoch_and_flags_only_the_live_view_rows() {
    let body = upload_body(6, &[event("a"), thinking()]);
    assert_eq!(
        body,
        json!({"worker_epoch": 6, "events": [
            {"payload": {"type": "assistant", "text": "a"}},
            {"payload": {"type": "assistant", "text": "t"}, "ephemeral": true},
        ]})
    );
    assert_eq!(
        Value::Array(vec![]),
        upload_body(1, &[])["events"],
        "an empty batch has no rows"
    );
}

#[test]
fn a_closed_queue_counts_what_was_waiting_and_what_came_after() {
    let mut queue = EventQueue::default();
    queue.enqueue(Vec::new());
    assert_eq!(queue.pending_count(), 0);
    queue.enqueue(vec![event("a"), event("b")]);
    assert_eq!(queue.pending_count(), 2);
    assert_eq!(queue.close(false), 2);
    assert_eq!(queue.pending_count(), 2, "the count is the one at the close");
    queue.enqueue(vec![event("c")]);
    assert_eq!(queue.handed_over_after_close(), 1);
    assert!(
        queue.take_undelivered().is_empty(),
        "a close without retain keeps nothing"
    );
    assert_eq!(queue.close(true), 0, "a second close does nothing");
}

#[test]
fn an_answer_with_no_batch_in_flight_changes_nothing() {
    let mut queue = EventQueue::default();
    assert_eq!(queue.keep_only(|_| false), 0);
    queue.failed(0.0, None, true);
    assert_eq!(queue.poll(), EventNext::Idle);
    assert!(queue.is_idle());
    assert!(queue.in_flight_events().is_empty());
}

#[test]
fn a_cut_batch_with_no_answer_is_measured_again_and_resent() -> TestResult {
    let mut queue = EventQueue::default();
    queue.enqueue(vec![event("a"), thinking()]);
    sends(queue.poll())?;
    assert_eq!(queue.keep_only(|event| !event.ephemeral), 1);
    queue.failed(0.0, None, true);
    assert_eq!(queue.poll(), EventNext::Pause(Duration::from_millis(500)));
    let retry = sends(queue.poll())?;
    assert_eq!(texts(&retry), vec!["a".to_owned()]);
    assert_eq!(retry.attempt, Attempt::Resend);
    Ok(())
}
