use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Mutex;

use buffa::MessageField;
use trogon_decider_runtime::{StreamEvent, StreamPosition};
use trogonai_proto::scheduler::schedules::v1;

use super::super::testkit::{
    InMemoryExecution, InMemoryKv, MemoryEventStore, malformed_stream_event, recorded_at, stream_event,
};
use super::*;
use super::{ReadyOutcome, resolve_ready_key};
use crate::commands::domain::MessageContent;
use crate::commands::domain::{
    Delivery, MessageEnvelope, Schedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus,
    ScheduleHeaders, ScheduleMessage,
};
use crate::processor::execution::checkpoints::ScheduleCheckpointStore;
use crate::processor::execution::execution_schedules::ExecutionScheduleWriter;
use crate::processor::execution::reconciliation::{DecodedScheduleEvent, ScheduleSubject};
use crate::processor::execution::reconciliation::{ScheduleKey, StreamRoutingId};
use crate::processor::execution::worker::ProcessorMetrics;

#[derive(Clone)]
enum Settlement {
    Ack,
    Term,
    Retry,
}

#[derive(Clone)]
struct MockMessage {
    log: Arc<Mutex<Vec<Settlement>>>,
    redelivery: bool,
    deliveries: i64,
}

fn mock_message(log: Arc<Mutex<Vec<Settlement>>>) -> MockMessage {
    MockMessage {
        log,
        redelivery: false,
        deliveries: 1,
    }
}

impl DeliveredMessage for MockMessage {
    fn is_redelivery(&self) -> bool {
        self.redelivery
    }

    fn delivery_count(&self) -> i64 {
        self.deliveries
    }

    async fn ack(&self) -> Result<(), String> {
        self.log.lock().unwrap().push(Settlement::Ack);
        Ok(())
    }
    async fn term(&self) -> Result<(), String> {
        self.log.lock().unwrap().push(Settlement::Term);
        Ok(())
    }
    async fn retry(&self) -> Result<(), String> {
        self.log.lock().unwrap().push(Settlement::Retry);
        Ok(())
    }
}

fn created(id: &str, schedule: Schedule) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCreated {
                schedule_id: id.to_string(),
                status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
                schedule: MessageField::some(v1::Schedule::try_from(&ScheduleEventSchedule::from(&schedule)).unwrap()),
                delivery: MessageField::some(
                    v1::Delivery::try_from(&ScheduleEventDelivery::from(
                        &Delivery::nats_event("agent.run").unwrap(),
                    ))
                    .unwrap(),
                ),
                message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&ScheduleMessage {
                    content: MessageContent::json("{}"),
                    headers: ScheduleHeaders::default(),
                }))),
            }
            .into(),
        ),
    }
}

fn paused(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::SchedulePaused {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

fn resumed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleResumed {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

fn removed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleRemoved {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

type Processor = ScheduleProcessor<InMemoryExecution, InMemoryExecution, InMemoryKv, MemoryEventStore>;

fn build() -> (Arc<Processor>, InMemoryKv, InMemoryExecution) {
    let kv = InMemoryKv::new();
    let execution = InMemoryExecution::new();
    let writer = ExecutionScheduleWriter::new(execution.clone(), execution.clone());
    let processor = ScheduleProcessor::new(
        writer,
        ScheduleCheckpointStore::new(kv.clone()),
        MemoryEventStore::default(),
        "SCHEDULER_SCHEDULE_EVENTS",
        Arc::new(ProcessorMetrics::new()),
    );
    (Arc::new(processor), kv, execution)
}

fn clock() -> Clock {
    Arc::new(recorded_at)
}

fn key_for_stream(id: &str) -> ScheduleKey {
    ScheduleKey::for_stream(&StreamRoutingId::from(id))
}

async fn drain(mut reports: mpsc::Receiver<DispatchReport>, expected: usize) -> Vec<DispatchReport> {
    let mut collected = Vec::new();
    while collected.len() < expected {
        match reports.recv().await {
            Some(report) => collected.push(report),
            None => break,
        }
    }
    collected
}

#[tokio::test]
async fn records_for_one_schedule_are_processed_in_submission_order() {
    let (processor, kv, execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    let key = key_for_stream(id);
    let subject = ScheduleSubject::execution(&key);
    let log = Arc::new(Mutex::new(Vec::new()));
    let message = mock_message(log.clone());

    let events = [
        created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap()),
        paused(id),
        resumed(id),
        removed(id),
    ];
    for (offset, event) in events.iter().enumerate() {
        handle
            .submit(stream_event(event, id, offset as u64 + 1), message.clone())
            .await
            .unwrap();
    }
    drop(handle);

    let reports = drain(reports, 4).await;
    join.await.unwrap();

    let positions: Vec<u64> = reports.iter().map(|r| r.stream_position.as_u64()).collect();
    assert_eq!(positions, vec![1, 2, 3, 4]);
    let outcomes: Vec<ProcessedOutcome> = reports.iter().map(|r| r.result.clone().unwrap()).collect();
    assert_eq!(
        outcomes,
        vec![
            ProcessedOutcome::Published,
            ProcessedOutcome::Purged,
            ProcessedOutcome::Published,
            ProcessedOutcome::Purged,
        ]
    );

    let stored = kv.contains(&format!("v1.{}", key.simple()));
    assert!(stored, "checkpoint record persists");
    assert_eq!(execution.scheduled_count(subject.as_str()), 0);
    assert_eq!(log.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn distinct_schedules_all_make_progress_and_lanes_are_evicted() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
        processor,
        clock(),
        DispatcherConfig {
            max_active_lanes: 2,
            channel_capacity: 64,
        },
    );

    let log = Arc::new(Mutex::new(Vec::new()));
    let message = mock_message(log);
    let ids = ["a", "b", "c", "d", "e"];
    for (offset, id) in ids.iter().enumerate() {
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle
            .submit(stream_event(&event, id, offset as u64 + 1), message.clone())
            .await
            .unwrap();
    }
    drop(handle);

    let reports = drain(reports, ids.len()).await;
    join.await.unwrap();

    assert_eq!(reports.len(), ids.len());
    assert!(
        reports
            .iter()
            .all(|r| matches!(r.result, Ok(ProcessedOutcome::Published)))
    );
}

#[tokio::test]
async fn transient_failure_is_retried_not_acked() {
    let kv = InMemoryKv::new();
    let execution = InMemoryExecution::new();
    execution.fail_next_publish();
    let writer = ExecutionScheduleWriter::new(execution.clone(), execution.clone());
    let processor = Arc::new(ScheduleProcessor::new(
        writer,
        ScheduleCheckpointStore::new(kv.clone()),
        MemoryEventStore::default(),
        "SCHEDULER_SCHEDULE_EVENTS",
        Arc::new(ProcessorMetrics::new()),
    ));
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    let log = Arc::new(Mutex::new(Vec::new()));
    let message = mock_message(log.clone());
    let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
    handle.submit(stream_event(&event, id, 1), message).await.unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();

    assert!(reports[0].result.is_err());
    assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Retry]));
    assert!(!kv.contains(&format!("v1.{}", key_for_stream(id).simple())));
}

#[tokio::test]
async fn paused_create_does_not_publish() {
    let (processor, kv, execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    let key = key_for_stream(id);
    let subject = ScheduleSubject::execution(&key);
    let log = Arc::new(Mutex::new(Vec::new()));

    let event = v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCreated {
                schedule_id: id.to_string(),
                status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Paused)),
                schedule: MessageField::some(
                    v1::Schedule::try_from(&ScheduleEventSchedule::from(
                        &Schedule::every(std::time::Duration::from_secs(30)).unwrap(),
                    ))
                    .unwrap(),
                ),
                delivery: MessageField::some(
                    v1::Delivery::try_from(&ScheduleEventDelivery::from(
                        &Delivery::nats_event("agent.run").unwrap(),
                    ))
                    .unwrap(),
                ),
                message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&ScheduleMessage {
                    content: MessageContent::json("{}"),
                    headers: ScheduleHeaders::default(),
                }))),
            }
            .into(),
        ),
    };
    handle
        .submit(stream_event(&event, id, 1), mock_message(log))
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();

    assert_eq!(reports[0].result.clone().unwrap(), ProcessedOutcome::StoredPaused);
    assert_eq!(execution.scheduled_count(subject.as_str()), 0);
    assert!(kv.contains(&format!("v1.{}", key.simple())));
}

#[tokio::test(start_paused = true)]
async fn full_report_channel_does_not_block_dispatcher_completion() {
    let (processor, _kv, _execution) = build();
    let (handle, join, _reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
        processor,
        clock(),
        DispatcherConfig {
            max_active_lanes: 2,
            channel_capacity: 1,
        },
    );

    let log = Arc::new(Mutex::new(Vec::new()));
    let message = mock_message(log);
    for (position, id) in ["a", "b"].into_iter().enumerate() {
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle
            .submit(stream_event(&event, id, position as u64 + 1), message.clone())
            .await
            .unwrap();
    }
    drop(handle);
    drop(_reports);

    tokio::time::timeout(std::time::Duration::from_secs(1), join)
        .await
        .expect("dispatcher must not wait for report receiver capacity")
        .unwrap();
}

#[tokio::test]
async fn active_lanes_is_zero_when_idle() {
    let (processor, _kv, _execution) = build();
    let (handle, join, _reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    assert_eq!(handle.active_lanes(), 0);
    drop(handle);
    join.await.unwrap();
}

#[tokio::test]
async fn malformed_event_is_terminated() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let log = Arc::new(Mutex::new(Vec::new()));
    handle
        .submit(malformed_stream_event(1), mock_message(log.clone()))
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();

    assert_eq!(reports[0].result.as_ref().unwrap(), &ProcessedOutcome::DurableFailure);
    assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Term]));
}

#[tokio::test]
async fn redelivered_record_is_processed() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let log = Arc::new(Mutex::new(Vec::new()));
    let id = "orders/created";
    let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
    handle
        .submit(
            stream_event(&event, id, 1),
            MockMessage {
                log: log.clone(),
                redelivery: true,
                deliveries: 2,
            },
        )
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();

    assert_eq!(reports[0].result.as_ref().unwrap(), &ProcessedOutcome::Published);
    assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Ack]));
}

#[tokio::test]
async fn missing_checkpoint_is_retried_below_the_delivery_ceiling() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    let log = Arc::new(Mutex::new(Vec::new()));
    handle
        .submit(stream_event(&paused(id), id, 2), mock_message(log.clone()))
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();

    assert!(reports[0].result.is_err());
    assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Retry]));
}

#[tokio::test]
async fn missing_checkpoint_is_poisoned_at_the_delivery_ceiling() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    let log = Arc::new(Mutex::new(Vec::new()));
    handle
        .submit(
            stream_event(&paused(id), id, 2),
            MockMessage {
                log: log.clone(),
                redelivery: true,
                deliveries: MISSING_CHECKPOINT_DELIVERY_CEILING,
            },
        )
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();

    assert_eq!(reports[0].result.as_ref().unwrap(), &ProcessedOutcome::DurableFailure);
    assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Term]));
}

#[derive(Clone)]
struct PanickingMessage;

impl DeliveredMessage for PanickingMessage {
    async fn ack(&self) -> Result<(), String> {
        panic!("settlement panic");
    }

    async fn term(&self) -> Result<(), String> {
        panic!("settlement panic");
    }

    async fn retry(&self) -> Result<(), String> {
        panic!("settlement panic");
    }
}

#[derive(Clone, Copy)]
struct FailingMessage;

impl DeliveredMessage for FailingMessage {
    async fn ack(&self) -> Result<(), String> {
        Err("ack failed".to_string())
    }

    async fn term(&self) -> Result<(), String> {
        Err("term failed".to_string())
    }

    async fn retry(&self) -> Result<(), String> {
        Err("retry failed".to_string())
    }
}

#[derive(Clone, Copy)]
struct DefaultDeliveryMessage;

impl DeliveredMessage for DefaultDeliveryMessage {
    async fn ack(&self) -> Result<(), String> {
        Ok(())
    }

    async fn term(&self) -> Result<(), String> {
        Ok(())
    }

    async fn retry(&self) -> Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
async fn delivered_message_defaults_describe_first_delivery() {
    let message = DefaultDeliveryMessage;

    assert!(!message.is_redelivery());
    assert_eq!(message.delivery_count(), 1);
    assert_eq!(message.ack().await, Ok(()));
    assert_eq!(message.term().await, Ok(()));
    assert_eq!(message.retry().await, Ok(()));
}

#[tokio::test]
async fn settlement_panic_does_not_abort_dispatcher() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, PanickingMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
    handle
        .submit(stream_event(&event, id, 1), PanickingMessage)
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.expect("dispatcher must survive settlement panics");

    assert!(reports[0].result.is_err());
    assert!(
        reports[0]
            .result
            .as_ref()
            .unwrap_err()
            .contains("message settlement panicked")
    );
}

#[derive(Clone, Copy)]
enum PanicSettlement {
    Term,
    Retry,
}

#[derive(Clone, Copy)]
struct SelectivePanickingMessage {
    panic_on: PanicSettlement,
}

impl DeliveredMessage for SelectivePanickingMessage {
    async fn ack(&self) -> Result<(), String> {
        let _ = self.panic_on;
        Ok(())
    }

    async fn term(&self) -> Result<(), String> {
        if matches!(self.panic_on, PanicSettlement::Term) {
            panic!("settlement panic");
        }
        Ok(())
    }

    async fn retry(&self) -> Result<(), String> {
        if matches!(self.panic_on, PanicSettlement::Retry) {
            panic!("settlement panic");
        }
        Ok(())
    }
}

#[tokio::test]
async fn selective_panicking_message_inert_paths_are_exercised() {
    let term = SelectivePanickingMessage {
        panic_on: PanicSettlement::Retry,
    };
    DeliveredMessage::term(&term).await.unwrap();
    let retry = SelectivePanickingMessage {
        panic_on: PanicSettlement::Term,
    };
    DeliveredMessage::retry(&retry).await.unwrap();
}

#[tokio::test]
async fn settlement_panic_on_term_does_not_abort_dispatcher() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, SelectivePanickingMessage>(processor, clock(), DispatcherConfig::default());

    handle
        .submit(
            malformed_stream_event(1),
            SelectivePanickingMessage {
                panic_on: PanicSettlement::Term,
            },
        )
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();
    assert!(reports[0].result.is_err());
}

#[tokio::test]
async fn settlement_panic_on_retry_does_not_abort_dispatcher() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, SelectivePanickingMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    handle
        .submit(
            stream_event(&paused(id), id, 1),
            SelectivePanickingMessage {
                panic_on: PanicSettlement::Retry,
            },
        )
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();
    assert!(reports[0].result.is_err());
}

#[tokio::test]
async fn processor_panic_is_recovered_and_settled() {
    let (processor, kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    kv.panic_on_next_entry();
    let log = Arc::new(Mutex::new(Vec::new()));
    handle
        .submit(
            stream_event(
                &created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap()),
                id,
                1,
            ),
            mock_message(log),
        )
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();

    assert_eq!(reports[0].result.as_ref().unwrap(), &ProcessedOutcome::DurableFailure);
}

#[tokio::test]
async fn poison_failure_panic_falls_back_to_retry() {
    let (processor, kv, _execution) = build();
    kv.panic_on_next_failure_record();

    let failure = processor.failure_record(
        &malformed_stream_event(9),
        PoisonReasonError::ProcessorPanic {
            stream_position: StreamPosition::try_new(9).unwrap(),
        },
    );
    let report = poison_record(
        processor,
        mock_message(Arc::new(Mutex::new(Vec::new()))),
        key_for_stream("malformed"),
        StreamPosition::try_new(9).unwrap(),
        failure,
    )
    .await;

    assert!(report.result.as_ref().unwrap_err().contains("recording a failure"));
}

#[tokio::test]
async fn finalize_report_merges_process_and_settlement_errors() {
    let error = finalize_report(
        PanickingMessage,
        Settle::Ack,
        StreamPosition::try_new(1).unwrap(),
        Err("process failed".to_string()),
    )
    .await
    .unwrap_err();

    assert!(error.contains("process failed"));
    assert!(error.contains("message settlement panicked"));
}

#[tokio::test]
async fn panicking_message_term_and_retry_also_panic() {
    let message = PanickingMessage;
    assert!(AssertUnwindSafe(message.term()).catch_unwind().await.is_err());
    assert!(AssertUnwindSafe(message.retry()).catch_unwind().await.is_err());
}

#[tokio::test]
async fn selective_panicking_message_ack_is_inert() {
    let message = SelectivePanickingMessage {
        panic_on: PanicSettlement::Term,
    };
    DeliveredMessage::ack(&message).await.unwrap();
}

#[tokio::test]
async fn settlement_error_is_reported() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, FailingMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    handle
        .submit(
            stream_event(
                &created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap()),
                id,
                1,
            ),
            FailingMessage,
        )
        .await
        .unwrap();
    drop(handle);

    let reports = drain(reports, 1).await;
    join.await.unwrap();

    assert!(reports[0].result.as_ref().unwrap_err().contains("ack failed"));
}

#[tokio::test]
async fn term_settlement_error_is_reported() {
    let error = finalize_report(
        FailingMessage,
        Settle::Term,
        StreamPosition::try_new(1).unwrap(),
        Ok(ProcessedOutcome::DurableFailure),
    )
    .await
    .unwrap_err();

    assert!(error.contains("term failed"));
}

#[tokio::test]
async fn retry_settlement_error_is_reported() {
    let error = finalize_report(
        FailingMessage,
        Settle::Retry,
        StreamPosition::try_new(1).unwrap(),
        Ok(ProcessedOutcome::DurableFailure),
    )
    .await
    .unwrap_err();

    assert!(error.contains("retry failed"));
}

#[test]
#[should_panic(expected = "max_active_lanes must be at least 1")]
fn zero_max_active_lanes_is_rejected() {
    let (processor, _kv, _execution) = build();
    let _ = spawn_dispatcher::<_, _, _, _, MockMessage>(
        processor,
        clock(),
        DispatcherConfig {
            max_active_lanes: 0,
            channel_capacity: 1,
        },
    );
}

#[tokio::test]
async fn dispatcher_finishes_when_report_receiver_is_dropped() {
    let (processor, _kv, _execution) = build();
    let (handle, join, _reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
        processor,
        clock(),
        DispatcherConfig {
            max_active_lanes: 3,
            channel_capacity: 1,
        },
    );

    let log = Arc::new(Mutex::new(Vec::new()));
    let message = mock_message(log);
    for (index, id) in ["a", "b", "c"].into_iter().enumerate() {
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle
            .submit(stream_event(&event, id, index as u64 + 1), message.clone())
            .await
            .unwrap();
    }
    drop(handle);
    drop(_reports);

    tokio::time::timeout(std::time::Duration::from_secs(1), join)
        .await
        .expect("dispatcher must finish when the report receiver is dropped")
        .unwrap();
}

#[tokio::test]
async fn slow_report_consumer_still_receives_all_reports() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
        processor,
        clock(),
        DispatcherConfig {
            max_active_lanes: 3,
            channel_capacity: 1,
        },
    );

    let log = Arc::new(Mutex::new(Vec::new()));
    let message = mock_message(log);
    for (index, id) in ["a", "b", "c"].into_iter().enumerate() {
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle
            .submit(stream_event(&event, id, index as u64 + 1), message.clone())
            .await
            .unwrap();
    }
    drop(handle);

    let collected = drain(reports, 3).await;
    join.await.unwrap();

    assert_eq!(collected.len(), 3);
    let positions: Vec<u64> = collected.iter().map(|report| report.stream_position.as_u64()).collect();
    assert_eq!(positions, vec![1, 2, 3]);
}

#[tokio::test]
async fn mismatched_stream_ids_for_one_schedule_stay_on_one_lane() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
        processor,
        clock(),
        DispatcherConfig {
            max_active_lanes: 2,
            channel_capacity: 8,
        },
    );

    let schedule = Schedule::every(std::time::Duration::from_secs(30)).unwrap();
    let id = "orders/created";
    let message = mock_message(Arc::new(Mutex::new(Vec::new())));

    let mut first = stream_event(&created(id, schedule.clone()), id, 1);
    first.stream_id = "wrong/create".to_string();
    handle.submit(first, message.clone()).await.unwrap();

    let second = stream_event(&created("other/schedule", schedule), "other/schedule", 2);
    handle.submit(second, message.clone()).await.unwrap();

    let mut third = stream_event(&paused(id), id, 3);
    third.stream_id = "wrong/pause".to_string();
    handle.submit(third, message).await.unwrap();
    drop(handle);

    let mut reports = drain(reports, 3).await;
    join.await.unwrap();
    reports.sort_by_key(|report| report.stream_position.as_u64());

    assert_eq!(
        reports
            .iter()
            .map(|report| report.stream_position.as_u64())
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(matches!(reports[0].result, Ok(ProcessedOutcome::DurableFailure)));
    assert!(matches!(reports[1].result, Ok(ProcessedOutcome::Published)));
    assert!(matches!(reports[2].result, Ok(ProcessedOutcome::DurableFailure)));
}

#[tokio::test]
async fn same_lane_resumes_after_the_first_record_finishes() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
        processor,
        clock(),
        DispatcherConfig {
            max_active_lanes: 1,
            channel_capacity: 8,
        },
    );

    let id = "orders/created";
    let log = Arc::new(Mutex::new(Vec::new()));
    let message = mock_message(log);
    for position in 1..=2 {
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle
            .submit(stream_event(&event, id, position), message.clone())
            .await
            .unwrap();
    }
    drop(handle);

    let reports = drain(reports, 2).await;
    join.await.unwrap();
    assert_eq!(
        reports
            .iter()
            .map(|report| report.stream_position.as_u64())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[tokio::test]
async fn dropped_report_receiver_stops_drain_early() {
    let (processor, _kv, _execution) = build();
    let (handle, join, reports) =
        spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

    let id = "orders/created";
    let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
    handle
        .submit(
            stream_event(&event, id, 1),
            mock_message(Arc::new(Mutex::new(Vec::new()))),
        )
        .await
        .unwrap();
    drop(handle);
    drop(reports);

    join.await.unwrap();
}

#[test]
fn flush_pending_reports_clears_queue_when_channel_is_closed() {
    let (tx, rx) = mpsc::channel::<DispatchReport>(4);
    drop(rx);

    let report = DispatchReport {
        stream_position: StreamPosition::try_new(1).unwrap(),
        lane: key_for_stream("flush-closed"),
        result: Ok(ProcessedOutcome::Published),
    };

    let mut pending_reports = std::collections::VecDeque::new();
    pending_reports.push_back(report);

    super::flush_pending_reports(&mut pending_reports, &tx);

    assert!(
        pending_reports.is_empty(),
        "flush_pending_reports must clear the queue when the channel receiver is dropped"
    );
}

#[test]
fn flush_pending_reports_drains_every_report_when_channel_has_capacity() {
    let (tx, mut rx) = mpsc::channel::<DispatchReport>(8);
    let lane = key_for_stream("flush-ok");
    let mut pending_reports = std::collections::VecDeque::new();
    for stream_position in 1..=4 {
        pending_reports.push_back(DispatchReport {
            stream_position: StreamPosition::try_new(stream_position).unwrap(),
            lane,
            result: Ok(ProcessedOutcome::Published),
        });
    }

    super::flush_pending_reports(&mut pending_reports, &tx);

    assert!(pending_reports.is_empty(), "every report must be drained on success");
    let mut drained = 0;
    while rx.try_recv().is_ok() {
        drained += 1;
    }
    assert_eq!(drained, 4);
}

#[tokio::test]
async fn drain_one_reserved_report_sends_front_report_through_permit() {
    let (tx, mut rx) = mpsc::channel::<DispatchReport>(4);
    let permit = tx.reserve().await.expect("permit must reserve");

    let lane = key_for_stream("drain-ok");
    let mut pending_reports = std::collections::VecDeque::new();
    pending_reports.push_back(DispatchReport {
        stream_position: StreamPosition::try_new(1).unwrap(),
        lane,
        result: Ok(ProcessedOutcome::Published),
    });

    super::drain_one_reserved_report(Ok(permit), &mut pending_reports);

    assert!(pending_reports.is_empty(), "front report must be consumed");
    assert!(rx.try_recv().is_ok(), "channel must have received the report");
}

#[test]
fn drain_one_reserved_report_clears_queue_when_channel_closed() {
    let lane = key_for_stream("drain-closed");
    let mut pending_reports = std::collections::VecDeque::new();
    pending_reports.push_back(DispatchReport {
        stream_position: StreamPosition::try_new(1).unwrap(),
        lane,
        result: Ok(ProcessedOutcome::Published),
    });

    super::drain_one_reserved_report(Err(mpsc::error::SendError(())), &mut pending_reports);

    assert!(pending_reports.is_empty(), "queue must be cleared on channel close");
}

#[test]
fn flush_pending_reports_stops_when_channel_is_full_without_clearing_queue() {
    let (tx, _rx) = mpsc::channel::<DispatchReport>(1);
    let lane = key_for_stream("flush-full");
    let blocker = DispatchReport {
        stream_position: StreamPosition::try_new(1).unwrap(),
        lane,
        result: Ok(ProcessedOutcome::Published),
    };
    tx.try_send(blocker).expect("seeded send must fit");

    let mut pending_reports = std::collections::VecDeque::new();
    pending_reports.push_back(DispatchReport {
        stream_position: StreamPosition::try_new(2).unwrap(),
        lane,
        result: Ok(ProcessedOutcome::Published),
    });

    super::flush_pending_reports(&mut pending_reports, &tx);

    assert_eq!(
        pending_reports.len(),
        1,
        "Full must stop the drain without consuming the still-pending report"
    );
}

#[test]
fn resolve_ready_key_returns_already_in_flight_when_key_is_in_flight() {
    let key = key_for_stream("in-flight-key");

    let mut in_flight = HashSet::new();
    in_flight.insert(key);

    let event = super::super::testkit::malformed_stream_event(1);
    let mut pending: HashMap<ScheduleKey, VecDeque<(StreamEvent, DecodedScheduleEvent, MockMessage)>> = HashMap::new();
    pending.entry(key).or_default().push_back((
        event,
        DecodedScheduleEvent::Undecoded,
        mock_message(Arc::new(Mutex::new(Vec::new()))),
    ));

    let outcome = resolve_ready_key(key, &in_flight, &mut pending);

    assert!(
        matches!(outcome, ReadyOutcome::AlreadyInFlight),
        "a key already in flight must be skipped without touching pending"
    );
    assert!(
        pending.contains_key(&key),
        "pending entry must be untouched when key is in flight"
    );
}

#[test]
fn resolve_ready_key_returns_empty_queue_when_pending_entry_is_absent() {
    let key = key_for_stream("no-pending-key");

    let in_flight: HashSet<ScheduleKey> = HashSet::new();
    let mut pending: HashMap<ScheduleKey, VecDeque<(StreamEvent, DecodedScheduleEvent, MockMessage)>> = HashMap::new();

    let outcome = resolve_ready_key(key, &in_flight, &mut pending);

    assert!(
        matches!(outcome, ReadyOutcome::EmptyQueue),
        "a key with no pending entries must be evicted"
    );
    assert!(
        !pending.contains_key(&key),
        "evicted key must not be present in pending after eviction"
    );
}

#[tokio::test]
async fn drain_stops_early_when_channel_closes_before_expected_count() {
    let (tx, rx) = mpsc::channel::<DispatchReport>(4);
    drop(tx);

    let collected = drain(rx, usize::MAX).await;

    assert!(
        collected.is_empty(),
        "drain must return immediately when the sender is dropped before any items are sent"
    );
}
