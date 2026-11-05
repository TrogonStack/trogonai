use super::*;

#[test]
fn an_empty_snapshot_is_all_zero() {
    let stats = WriterStats::default();
    assert_eq!(stats.dropped(AuditClass::Must, FailureReason::BufferFull), 0);
    assert_eq!(stats.dropped_total(AuditClass::Must), 0);
    assert_eq!(stats.refused(FailureReason::Degraded), 0);
    assert_eq!(stats.refused_total(), 0);
    assert!(!stats.degraded());
    assert_eq!(stats.retry_buffered(), 0);
    assert_eq!(stats.best_effort_queued(), 0);
}

#[test]
fn totals_sum_per_class_and_reason() {
    let mut stats = WriterStats::default();
    stats.dropped.insert((AuditClass::Must, FailureReason::BufferFull), 2);
    stats.dropped.insert((AuditClass::Must, FailureReason::Timeout), 3);
    stats
        .dropped
        .insert((AuditClass::BestEffort, FailureReason::QueueFull), 7);
    stats.refused.insert(FailureReason::Degraded, 4);
    stats.refused.insert(FailureReason::Timeout, 1);
    assert_eq!(stats.dropped(AuditClass::Must, FailureReason::BufferFull), 2);
    assert_eq!(stats.dropped_total(AuditClass::Must), 5);
    assert_eq!(stats.dropped_total(AuditClass::BestEffort), 7);
    assert_eq!(stats.refused(FailureReason::Degraded), 4);
    assert_eq!(stats.refused_total(), 5);
}
