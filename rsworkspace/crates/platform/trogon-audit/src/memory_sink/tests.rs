use std::time::Duration;

use super::*;
use crate::fixtures::{allow, deny, draft_of, head_at, sealed_at, signed_with, stamper};

#[tokio::test]
async fn stores_in_order_numbers_from_one_and_counts_attempts() {
    let stamper = stamper();
    let sink = MemorySink::new();
    let first = sealed_at(allow(&stamper), 1);
    let second = sealed_at(deny(&stamper), 2);
    assert_eq!(sink.publish(&first).await, Ok(StoredAck::new(STREAM_AUDIT, 1, false)));
    assert_eq!(sink.publish(&second).await, Ok(StoredAck::new(STREAM_AUDIT, 2, false)));
    assert_eq!(sink.stored(), vec![first, second]);
    assert_eq!(sink.attempts(), 2);
}

#[tokio::test]
async fn acknowledges_a_duplicate_at_the_first_copys_sequence() {
    let stamper = stamper();
    let sink = MemorySink::default();
    let record = sealed_at(allow(&stamper), 1);
    sink.publish(&sealed_at(deny(&stamper), 2)).await.unwrap();
    sink.publish(&record).await.unwrap();
    assert_eq!(sink.publish(&record).await, Ok(StoredAck::new(STREAM_AUDIT, 2, true)));
    assert_eq!(sink.stored().len(), 2);
    assert_eq!(sink.attempts(), 3);
}

#[tokio::test(start_paused = true)]
async fn stores_a_record_again_once_its_id_leaves_the_duplicate_window() {
    let stamper = stamper();
    let sink = MemorySink::new().with_duplicate_window(Duration::from_secs(10));
    let record = sealed_at(allow(&stamper), 1);
    sink.publish(&record).await.unwrap();

    tokio::time::advance(Duration::from_secs(10)).await;

    assert_eq!(sink.publish(&record).await, Ok(StoredAck::new(STREAM_AUDIT, 2, false)));
    assert_eq!(sink.stored(), vec![record.clone(), record]);
}

#[tokio::test(start_paused = true)]
async fn remembers_a_record_id_for_the_audit_streams_window_by_default() {
    let stamper = stamper();
    let sink = MemorySink::new();
    let record = sealed_at(allow(&stamper), 1);
    sink.publish(&record).await.unwrap();

    tokio::time::advance(STREAM_DUPLICATE_WINDOW - Duration::from_millis(1)).await;

    assert!(sink.publish(&record).await.unwrap().duplicate());
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test]
async fn reads_back_what_it_stored_with_the_subject_and_headers() {
    let stamper = stamper();
    let sink = MemorySink::new();
    let record = sealed_at(deny(&stamper), 3);
    sink.publish(&record).await.unwrap();

    let stored = sink.stored_records();

    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].stream_sequence(), 1);
    assert_eq!(stored[0].payload(), record.payload());
    assert_eq!(stored[0].subject(), Some(record.subject()));
    assert_eq!(stored[0].sequence(), Some("3"));
    assert!(stored[0].has_metadata());
}

#[tokio::test]
async fn fails_the_next_publishes_and_stores_nothing() {
    let stamper = stamper();
    let sink = MemorySink::new();
    sink.fail_next(2, SinkError::StreamFull);
    let record = sealed_at(allow(&stamper), 1);
    assert_eq!(sink.publish(&record).await, Err(SinkError::StreamFull));
    assert_eq!(sink.publish(&record).await, Err(SinkError::StreamFull));
    assert!(sink.stored().is_empty());
    sink.publish(&record).await.unwrap();
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test]
async fn fails_everything_until_it_recovers() {
    let stamper = stamper();
    let sink = MemorySink::new();
    sink.fail_all(SinkError::Unreachable);
    sink.fail_next(1, SinkError::Timeout);
    let record = sealed_at(allow(&stamper), 1);
    for _ in 0..3 {
        assert_eq!(sink.publish(&record).await, Err(SinkError::Unreachable));
    }
    sink.recover();
    sink.publish(&record).await.unwrap();
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test]
async fn a_lost_ack_stores_the_record_and_reports_a_timeout() {
    let stamper = stamper();
    let sink = MemorySink::new();
    sink.lose_next_acks(1);
    let record = sealed_at(allow(&stamper), 1);
    assert_eq!(sink.publish(&record).await, Err(SinkError::Timeout));
    assert_eq!(sink.stored().len(), 1);
    assert_eq!(sink.publish(&record).await, Ok(StoredAck::new(STREAM_AUDIT, 1, true)));
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test]
async fn recover_clears_a_pending_lost_ack() {
    let stamper = stamper();
    let sink = MemorySink::new();
    sink.lose_next_acks(5);
    sink.recover();
    sink.publish(&sealed_at(allow(&stamper), 1)).await.unwrap();
}

#[tokio::test]
async fn a_held_publish_counts_as_an_attempt_and_stores_once_released() {
    let stamper = stamper();
    let sink = MemorySink::new();
    let record = sealed_at(allow(&stamper), 1);
    sink.hold();

    let publishing = tokio::spawn({
        let sink = sink.clone();
        let record = record.clone();
        async move { sink.publish(&record).await }
    });
    while sink.attempts() == 0 {
        tokio::task::yield_now().await;
    }
    assert!(sink.stored().is_empty());

    sink.release();

    assert_eq!(publishing.await.unwrap(), Ok(StoredAck::new(STREAM_AUDIT, 1, false)));
    assert_eq!(sink.stored(), vec![record]);
}

#[tokio::test]
async fn stores_checkpoints_apart_from_records_and_numbers_them_from_one() {
    let stamper = stamper();
    let sink = MemorySink::new();
    let first = signed_with(1, &draft_of(head_at(1)));
    let second = signed_with(1, &draft_of(head_at(2)));
    sink.publish(&sealed_at(allow(&stamper), 1)).await.unwrap();

    assert_eq!(
        sink.publish_checkpoint(&first).await,
        Ok(StoredAck::new(STREAM_AUDIT_CHECKPOINT, 1, false))
    );
    assert_eq!(
        sink.publish_checkpoint(&second).await,
        Ok(StoredAck::new(STREAM_AUDIT_CHECKPOINT, 2, false))
    );
    assert_eq!(sink.checkpoints(), vec![first, second]);
    assert_eq!(sink.stored().len(), 1);
    assert_eq!(sink.attempts(), 3);
}

#[tokio::test]
async fn a_checkpoint_meets_the_same_injected_failures() {
    let sink = MemorySink::new();
    let checkpoint = signed_with(1, &draft_of(head_at(1)));
    sink.fail_next(1, SinkError::StreamFull);

    assert_eq!(sink.publish_checkpoint(&checkpoint).await, Err(SinkError::StreamFull));
    sink.fail_all(SinkError::Unreachable);
    assert_eq!(sink.publish_checkpoint(&checkpoint).await, Err(SinkError::Unreachable));
    assert!(sink.checkpoints().is_empty());
    assert_eq!(sink.attempts(), 2);
}
