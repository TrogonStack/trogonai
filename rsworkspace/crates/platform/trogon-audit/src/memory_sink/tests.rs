use super::*;
use crate::fixtures::{allow, deny, stamper, writer_id};

fn sealed(record: crate::record::Record, sequence: u64) -> SealedRecord {
    SealedRecord::seal(record, &writer_id(), sequence)
}

#[tokio::test]
async fn stores_in_order_and_counts_attempts() {
    let stamper = stamper();
    let sink = MemorySink::new();
    let first = sealed(allow(&stamper), 1);
    let second = sealed(deny(&stamper), 2);
    sink.publish(&first).await.unwrap();
    sink.publish(&second).await.unwrap();
    assert_eq!(sink.stored(), vec![first, second]);
    assert_eq!(sink.attempts(), 2);
}

#[tokio::test]
async fn keeps_one_copy_of_a_record_id() {
    let stamper = stamper();
    let sink = MemorySink::new();
    let record = sealed(allow(&stamper), 1);
    sink.publish(&record).await.unwrap();
    sink.publish(&record).await.unwrap();
    assert_eq!(sink.stored().len(), 1);
    assert_eq!(sink.attempts(), 2);
}

#[tokio::test]
async fn fails_the_next_publishes_and_stores_nothing() {
    let stamper = stamper();
    let sink = MemorySink::new();
    sink.fail_next(2, SinkError::StreamFull);
    let record = sealed(allow(&stamper), 1);
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
    let record = sealed(allow(&stamper), 1);
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
    let record = sealed(allow(&stamper), 1);
    assert_eq!(sink.publish(&record).await, Err(SinkError::Timeout));
    assert_eq!(sink.stored().len(), 1);
    sink.publish(&record).await.unwrap();
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test]
async fn recover_clears_a_pending_lost_ack() {
    let stamper = stamper();
    let sink = MemorySink::new();
    sink.lose_next_acks(5);
    sink.recover();
    sink.publish(&sealed(allow(&stamper), 1)).await.unwrap();
}
