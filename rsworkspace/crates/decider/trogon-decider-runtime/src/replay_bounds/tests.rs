use chrono::{DateTime, Utc};

use super::*;
use crate::{Event, EventId, Headers};

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

fn limit(value: u64) -> ReplayLimit {
    ReplayLimit::try_new(value).expect("test replay limit must be non-zero")
}

fn chunk_size(value: u64) -> ReplayChunkSize {
    ReplayChunkSize::try_new(value).expect("test chunk size must be non-zero")
}

fn stream_events(positions: &[u64]) -> Vec<StreamEvent> {
    positions
        .iter()
        .map(|sequence| StreamEvent {
            stream_id: "alpha".to_string(),
            event: Event {
                id: EventId::new(uuid::Uuid::nil()),
                r#type: "test".to_string(),
                content: Vec::new(),
                headers: Headers::empty(),
            },
            stream_position: position(*sequence),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("timestamp is in range"),
        })
        .collect()
}

#[test]
fn an_unbounded_replay_reads_the_whole_stream() {
    assert_eq!(ReplayBounds::default().read_bound(0), None);
}

#[test]
fn a_limit_alone_reads_one_past_its_remaining_allowance() {
    let bounds = ReplayBounds::new(Some(limit(10)), None);

    assert_eq!(bounds.read_bound(0), Some(11));
    assert_eq!(bounds.read_bound(8), Some(3));
    assert_eq!(
        bounds.read_bound(10),
        Some(1),
        "the probe survives an exhausted allowance, so the overrun is still detectable"
    );
    assert_eq!(bounds.read_bound(u64::MAX), Some(1));
}

#[test]
fn a_chunk_size_alone_reads_the_same_amount_every_time() {
    let bounds = ReplayBounds::new(None, Some(chunk_size(4)));

    assert_eq!(bounds.read_bound(0), Some(4));
    assert_eq!(bounds.read_bound(1_000), Some(4));
}

#[test]
fn the_tighter_of_the_two_bounds_wins() {
    let bounds = ReplayBounds::new(Some(limit(10)), Some(chunk_size(4)));

    assert_eq!(
        bounds.read_bound(0),
        Some(4),
        "the chunk is tighter while there is room"
    );
    assert_eq!(
        bounds.read_bound(8),
        Some(3),
        "the remaining allowance is tighter near the limit"
    );
}

#[test]
fn an_unchunked_cursor_never_asks_for_a_second_read() {
    let mut cursor = ReplayCursor::new(ReplayBounds::new(Some(limit(10)), None), Some(position(9)));
    cursor.advance(&stream_events(&[1, 2]));

    assert!(cursor.next_read().is_none());
}

#[test]
fn a_chunked_cursor_walks_from_the_last_event_it_replayed() {
    let mut cursor = ReplayCursor::new(ReplayBounds::new(None, Some(chunk_size(2))), Some(position(5)));

    assert_eq!(cursor.advance(&stream_events(&[1, 2])), 2);
    assert_eq!(
        cursor.next_read().transpose().unwrap(),
        Some(ReadFrom::Position(position(3)))
    );

    assert_eq!(cursor.advance(&stream_events(&[3, 5])), 4);
    assert_eq!(
        cursor.next_read().transpose().unwrap(),
        None,
        "reaching the pinned tail ends the walk, gaps in between or not"
    );
}

#[test]
fn a_chunked_cursor_stops_on_a_read_that_returned_nothing() {
    let cursor = ReplayCursor::new(ReplayBounds::new(None, Some(chunk_size(2))), Some(position(5)));

    assert!(
        cursor.next_read().is_none(),
        "asking again without advancing would fetch the same nothing forever"
    );
}

#[test]
fn a_read_that_reported_no_tail_is_folded_as_it_came_back() {
    let mut cursor = ReplayCursor::new(ReplayBounds::new(None, Some(chunk_size(2))), None);
    let mut events = stream_events(&[1, 2]);

    cursor.truncate_to_tail(&mut events);
    cursor.advance(&events);

    assert_eq!(
        events.len(),
        2,
        "dropping them would turn a store's contradiction into a wrong answer"
    );
    assert_eq!(cursor.replayed_event_count(), 2);
    assert!(cursor.next_read().is_none(), "there is no tail to walk toward");
}

#[test]
fn events_past_the_pinned_tail_are_left_for_the_next_execution() {
    let cursor = ReplayCursor::new(ReplayBounds::new(None, Some(chunk_size(4))), Some(position(3)));
    let mut events = stream_events(&[2, 3, 4, 5]);

    cursor.truncate_to_tail(&mut events);

    assert_eq!(
        events
            .iter()
            .map(|event| event.stream_position.as_u64())
            .collect::<Vec<_>>(),
        vec![2, 3],
        "folding past the pin would decide from history the append is not guarded against"
    );
}
