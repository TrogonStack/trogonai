use serde_json::{Value, json};
use tokio::time::Instant;

use super::*;

fn frame(value: Value) -> Frame {
    value.as_object().cloned().unwrap_or_default()
}

fn delta(index: u64, text: &str) -> Value {
    json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": text}}})
}

#[tokio::test(start_paused = true)]
async fn a_value_that_is_not_an_object_is_ignored() {
    let mut window = StreamEvents::default();
    assert!(window.write(json!("text"), Instant::now()).is_empty());
    assert!(window.due().is_none());
}

#[test]
fn truthiness_follows_javascript() {
    assert!(!truthy(&Value::Null));
    assert!(truthy(&json!(true)));
    assert!(!truthy(&json!(false)));
    assert!(truthy(&json!(2)));
    assert!(!truthy(&json!(0)));
    assert!(truthy(&json!("a")));
    assert!(!truthy(&json!("")));
    assert!(truthy(&json!([])));
    assert!(truthy(&json!({})));
}

#[test]
fn a_sum_of_tokens_prints_without_a_fraction() {
    assert_eq!(number(3.0), json!(3));
    assert_eq!(number(1.5), json!(1.5));
}

#[test]
fn a_chunk_weighs_its_escapes() {
    assert_eq!(escaped_len("ab"), 2);
    assert_eq!(escaped_len("\"\n"), 4);
    assert_eq!(escaped_len("\u{e9}"), 2);
}

#[tokio::test(start_paused = true)]
async fn deltas_merge_when_the_window_is_taken() {
    let mut window = StreamEvents::default();
    let now = Instant::now();
    assert!(window.write(delta(0, "a"), now).is_empty());
    assert_eq!(window.due(), Some(now + WINDOW));
    assert!(window.write(delta(0, "b"), now).is_empty());
    let sent = window.take();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].ephemeral);
    assert_eq!(sent[0].payload["event"]["delta"]["text"], "ab");
    assert!(window.due().is_none());
}

#[tokio::test(start_paused = true)]
async fn another_block_is_not_merged() {
    let mut window = StreamEvents::default();
    let now = Instant::now();
    window.write(delta(0, "a"), now);
    window.write(delta(1, "b"), now);
    assert_eq!(window.take().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_text_row_waits_and_close_hands_it_back() {
    let mut window = StreamEvents::default();
    let now = Instant::now();
    window.write(delta(0, "a"), now);
    let row = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "hi"}]}});
    assert!(window.write(row, now).is_empty());
    let kept = window.close();
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].ephemeral);
    assert!(window.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_row_with_a_tool_call_does_not_wait() {
    let mut window = StreamEvents::default();
    let now = Instant::now();
    window.write(delta(0, "a"), now);
    let row = json!({"type": "assistant", "message": {"content": [{"type": "tool_use"}]}});
    assert_eq!(window.write(row, now).len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_block_start_sends_a_waiting_row() {
    let mut window = StreamEvents::default();
    let now = Instant::now();
    window.write(delta(0, "a"), now);
    let row = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "hi"}]}});
    window.write(row, now);
    let start = json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 1}});
    assert_eq!(window.write(start, now).len(), 3);
}

#[test]
fn transformations_are_stripped_from_a_message() {
    let mut start = frame(json!({"event": {"type": "message_start", "message": {"input_transformations": [1]}}}));
    strip_transformations(&mut start);
    assert!(start["event"]["message"].get("input_transformations").is_none());
    let mut changed = frame(json!({"event": {"type": "message_delta", "input_transformations": [1]}}));
    strip_transformations(&mut changed);
    assert!(changed["event"].get("input_transformations").is_none());
    let mut other = frame(json!({"event": {"type": "message_stop", "input_transformations": [1]}}));
    strip_transformations(&mut other);
    assert!(other["event"].get("input_transformations").is_some());
    let mut bare = frame(json!({"type": "stream_event"}));
    strip_transformations(&mut bare);
}
