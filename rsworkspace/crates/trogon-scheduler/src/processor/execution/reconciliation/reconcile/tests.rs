use std::time::Duration;

    use super::*;
    use crate::commands::domain::{Delivery, MessageContent, ScheduleHeaders, ScheduleMessage};

    fn schedule_id(raw: &str) -> ScheduleId {
        ScheduleId::parse(raw).unwrap()
    }

    fn instant(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw).unwrap().with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        instant("2026-06-03T00:00:00Z")
    }

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).unwrap()
    }

    fn message() -> ScheduleMessage {
        ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::default(),
        }
    }

    fn created(id: &str, status: ScheduleEventStatus, schedule: Schedule) -> ScheduleChange {
        ScheduleChange::Created {
            schedule_id: schedule_id(id),
            definition: Box::new(ScheduleDefinition {
                status,
                schedule,
                delivery: Delivery::nats_event("agent.run").unwrap(),
                message: message(),
            }),
        }
    }

    fn occurrence_recorded(id: &str, occurrence_sequence: u64, occurrence_at: &str) -> ScheduleChange {
        ScheduleChange::OccurrenceRecorded {
            schedule_id: schedule_id(id),
            occurrence_sequence: ScheduleOccurrenceSequence::try_new(occurrence_sequence).unwrap(),
            occurrence_at: instant(occurrence_at),
        }
    }

    fn occurrence_scheduled(id: &str, occurrence_at: &str) -> ScheduleChange {
        ScheduleChange::OccurrenceScheduled {
            schedule_id: schedule_id(id),
            occurrence_sequence: ScheduleOccurrenceSequence::try_new(2).unwrap(),
            occurrence_at: instant(occurrence_at),
        }
    }

    fn completed(id: &str) -> ScheduleChange {
        ScheduleChange::Completed {
            schedule_id: schedule_id(id),
        }
    }

    fn scheduled_record(id: &str, schedule: Schedule) -> ScheduleCheckpointRecord {
        reconcile(
            None,
            &created(id, ScheduleEventStatus::Scheduled, schedule),
            position(1),
            Some("event-1"),
            now(),
        )
        .unwrap()
        .next_checkpoint
    }

    #[test]
    fn enabled_creation_with_future_at_publishes_schedule() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Scheduled,
                Schedule::At {
                    at: instant("2999-01-01T00:00:00Z"),
                },
            ),
            position(1),
            Some("event-1"),
            now(),
        )
        .unwrap();

        assert!(matches!(reconciliation.action, ReconcileAction::Publish(_)));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Published);
        assert_eq!(reconciliation.next_checkpoint.last_applied_stream_position, position(1));
        assert_eq!(
            reconciliation.next_checkpoint.last_applied_event_id.as_deref(),
            Some("event-1")
        );
    }

    #[test]
    fn enabled_creation_with_at_within_the_grace_window_still_publishes() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Scheduled,
                Schedule::At {
                    at: now() - PAST_AT_GRACE + chrono::Duration::seconds(1),
                },
            ),
            position(1),
            Some("event-1"),
            now(),
        )
        .unwrap();

        assert!(matches!(reconciliation.action, ReconcileAction::Publish(_)));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Published);
    }

    #[test]
    fn enabled_creation_with_at_at_the_grace_boundary_expires() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Scheduled,
                Schedule::At {
                    at: now() - PAST_AT_GRACE,
                },
            ),
            position(1),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Expired);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Expired);
    }

    #[test]
    fn enabled_creation_with_past_at_expires_without_publishing() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Scheduled,
                Schedule::At {
                    at: instant("2000-01-01T00:00:00Z"),
                },
            ),
            position(1),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Expired);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Expired);
    }

    #[test]
    fn enabled_creation_with_rrule_arms_the_next_occurrence() {
        let rrule = Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap();
        let reconciliation = reconcile(
            None,
            &created("recurring", ScheduleEventStatus::Scheduled, rrule),
            position(1),
            None,
            now(),
        )
        .unwrap();

        // RRULE recurrence is owned by the aggregate: creation hands off to the
        // arm command instead of expanding the rule in the processor.
        assert_eq!(
            reconciliation.action,
            ReconcileAction::ArmNext {
                schedule_id: schedule_id("recurring"),
                now: now(),
            }
        );
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Published);
    }

    #[test]
    fn recorded_occurrence_dispatches_the_user_message() {
        let current = scheduled_record(
            "recurring",
            Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
        );

        let dispatch = reconcile(
            Some(&current),
            &occurrence_recorded("recurring", 1, "2026-06-03T00:00:00Z"),
            position(2),
            Some("event-2"),
            now(),
        )
        .unwrap();

        let expected_dispatch = DispatchRequest::build_occurrence(
            &schedule_id("recurring"),
            ScheduleOccurrenceSequence::try_new(1).unwrap(),
            instant("2026-06-03T00:00:00Z"),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();
        assert_eq!(dispatch.action, ReconcileAction::Dispatch(expected_dispatch));
        assert_eq!(dispatch.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(dispatch.next_checkpoint.last_outcome, ReconcileOutcome::Published);
        assert_eq!(dispatch.next_checkpoint.last_applied_stream_position, position(2));
    }

    #[test]
    fn recorded_occurrence_dispatches_while_checkpoint_is_paused() {
        let mut current = scheduled_record(
            "recurring",
            Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
        );
        current.status = ScheduleStatus::Paused;

        let dispatch = reconcile(
            Some(&current),
            &occurrence_recorded("recurring", 1, "2026-06-03T00:00:00Z"),
            position(2),
            Some("event-2"),
            now(),
        )
        .unwrap();

        let expected_dispatch = DispatchRequest::build_occurrence(
            &schedule_id("recurring"),
            ScheduleOccurrenceSequence::try_new(1).unwrap(),
            instant("2026-06-03T00:00:00Z"),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();
        assert_eq!(dispatch.action, ReconcileAction::Dispatch(expected_dispatch));
        assert_eq!(dispatch.next_checkpoint.status, ScheduleStatus::Paused);
        assert_eq!(dispatch.next_checkpoint.last_outcome, ReconcileOutcome::Published);
        assert_eq!(dispatch.next_checkpoint.last_applied_stream_position, position(2));
    }

    #[test]
    fn scheduled_occurrence_publishes_the_planned_wakeup() {
        let current = scheduled_record(
            "recurring",
            Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
        );

        let continuation = reconcile(
            Some(&current),
            &occurrence_scheduled("recurring", "2026-06-04T00:00:00Z"),
            position(3),
            Some("event-3"),
            now(),
        )
        .unwrap();

        let expected = ScheduleRequest::build_rrule_wakeup(
            &schedule_id("recurring"),
            instant("2026-06-04T00:00:00Z"),
            &Delivery::nats_event("agent.run").unwrap(),
        )
        .unwrap();
        assert_eq!(continuation.action, ReconcileAction::Publish(expected));
        assert_eq!(continuation.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(continuation.next_checkpoint.last_outcome, ReconcileOutcome::Published);
    }

    #[test]
    fn completed_event_purges_and_expires() {
        let current = scheduled_record(
            "recurring",
            Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=1", None).unwrap(),
        );

        let completion = reconcile(
            Some(&current),
            &completed("recurring"),
            position(3),
            Some("event-3"),
            now(),
        )
        .unwrap();

        assert_eq!(completion.action, ReconcileAction::Purge(current.subject()));
        assert_eq!(completion.next_checkpoint.status, ScheduleStatus::Expired);
        assert_eq!(completion.next_checkpoint.last_outcome, ReconcileOutcome::Expired);
    }

    #[test]
    fn completed_event_noops_for_non_rrule_checkpoints() {
        let current = scheduled_record("recurring", Schedule::every(Duration::from_secs(30)).unwrap());

        let completion = reconcile(
            Some(&current),
            &completed("recurring"),
            position(3),
            Some("event-3"),
            now(),
        )
        .unwrap();

        assert_eq!(completion.action, ReconcileAction::CheckpointOnly);
        assert_eq!(completion.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(
            completion.next_checkpoint.last_outcome,
            ReconcileOutcome::DuplicateStale
        );
    }

    #[test]
    fn completed_event_expires_paused_rrule_checkpoints() {
        let mut current = scheduled_record(
            "recurring",
            Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=1", None).unwrap(),
        );
        current.status = ScheduleStatus::Paused;

        let completion = reconcile(
            Some(&current),
            &completed("recurring"),
            position(3),
            Some("event-3"),
            now(),
        )
        .unwrap();

        assert_eq!(completion.action, ReconcileAction::Purge(current.subject()));
        assert_eq!(completion.next_checkpoint.status, ScheduleStatus::Expired);
        assert_eq!(completion.next_checkpoint.last_outcome, ReconcileOutcome::Expired);
    }

    #[test]
    fn recorded_occurrence_noops_for_non_rrule_checkpoints() {
        let current = scheduled_record("recurring", Schedule::every(Duration::from_secs(30)).unwrap());

        let continuation = reconcile(
            Some(&current),
            &occurrence_recorded("recurring", 1, "2026-06-03T00:00:00Z"),
            position(2),
            Some("event-2"),
            now(),
        )
        .unwrap();

        assert_eq!(continuation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(
            continuation.next_checkpoint.last_outcome,
            ReconcileOutcome::DuplicateStale
        );
    }

    #[test]
    fn resume_of_rrule_arms_the_next_occurrence() {
        let current = scheduled_record(
            "recurring",
            Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
        );

        let reconciliation = reconcile(
            Some(&current),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("recurring"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(
            reconciliation.action,
            ReconcileAction::ArmNext {
                schedule_id: schedule_id("recurring"),
                now: now(),
            }
        );
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
    }

    #[test]
    fn paused_creation_stores_checkpoint_without_publishing() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Paused,
                Schedule::every(Duration::from_secs(30)).unwrap(),
            ),
            position(1),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Paused);
        assert_eq!(
            reconciliation.next_checkpoint.last_outcome,
            ReconcileOutcome::StoredPaused
        );
    }

    #[test]
    fn pause_purges_the_subject() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let reconciliation = reconcile(
            Some(&current),
            &ScheduleChange::Paused {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::Purge(current.subject()));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Paused);
        assert_eq!(reconciliation.next_checkpoint.last_applied_stream_position, position(2));
    }

    #[test]
    fn resume_republishes_from_the_stored_definition() {
        let paused = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Paused,
                Schedule::every(Duration::from_secs(30)).unwrap(),
            ),
            position(1),
            None,
            now(),
        )
        .unwrap()
        .next_checkpoint;

        let reconciliation = reconcile(
            Some(&paused),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap();

        assert!(matches!(reconciliation.action, ReconcileAction::Publish(_)));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
    }

    #[test]
    fn resume_from_removed_checkpoint_reports_unrecoverable_checkpoint() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let removed = reconcile(
            Some(&current),
            &ScheduleChange::Removed {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap()
        .next_checkpoint;

        let error = reconcile(
            Some(&removed),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(3),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::UnrecoverableCheckpoint { .. }));
    }

    #[test]
    fn resume_from_corrupt_placeholder_checkpoint_reports_unrecoverable_checkpoint() {
        let mut current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        current.status = ScheduleStatus::Paused;
        current.delivery = Delivery::nats_event(CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE).unwrap();
        current.last_outcome = ReconcileOutcome::Purged;

        let error = reconcile(
            Some(&current),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(3),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::UnrecoverableCheckpoint { .. }));
    }

    #[test]
    fn corrupt_placeholder_route_is_unclaimable_by_user_schedules() {
        assert!(ScheduleSubject::is_scheduler_internal(
            CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE
        ));
    }

    #[test]
    fn resume_rejects_a_checkpoint_for_a_different_schedule() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());

        let error = reconcile(
            Some(&current),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("invoices"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::MissingCheckpoint { .. }));
    }

    #[test]
    fn remove_purges_and_marks_removed() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let reconciliation = reconcile(
            Some(&current),
            &ScheduleChange::Removed {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap();

        assert!(matches!(reconciliation.action, ReconcileAction::Purge(_)));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Removed);
    }

    #[test]
    fn purge_rejects_a_checkpoint_for_a_different_schedule() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());

        let error = reconcile(
            Some(&current),
            &ScheduleChange::Removed {
                schedule_id: schedule_id("invoices"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::MissingCheckpoint { .. }));
    }

    #[test]
    fn schedule_changes_without_prior_checkpoint_report_missing_checkpoint() {
        let error = reconcile(
            None,
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::MissingCheckpoint { .. }));
    }

    #[test]
    fn reconcile_errors_display_and_expose_sources() {
        let missing = ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id("orders"),
        };
        assert_eq!(
            missing.to_string(),
            "no scheduler checkpoint exists for schedule 'orders'"
        );
        assert!(std::error::Error::source(&missing).is_none());

        let unrecoverable = ReconcileError::UnrecoverableCheckpoint {
            schedule_id: schedule_id("orders"),
        };
        assert_eq!(
            unrecoverable.to_string(),
            "scheduler checkpoint for schedule 'orders' cannot be resumed"
        );
        assert!(std::error::Error::source(&unrecoverable).is_none());

        let request = ReconcileError::ScheduleRequest {
            source: ScheduleRequestError::UnsupportedSchedule,
        };
        assert_eq!(
            request.to_string(),
            "schedule request failed: schedule kind is not supported by NATS message scheduling"
        );
        assert!(std::error::Error::source(&request).is_some());
    }

    #[test]
    fn resume_with_invalid_delivery_target_fails_schedule_request() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let subject = current.subject();
        let mut current = current;
        current.delivery = Delivery::nats_event(subject.as_str()).unwrap();

        let error = reconcile(
            Some(&current),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(3),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::ScheduleRequest { .. }));
    }

    #[test]
    fn stale_records_are_a_no_op_that_preserves_the_definition() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let reconciliation = reconcile(
            Some(&current),
            &ScheduleChange::Removed {
                schedule_id: schedule_id("orders"),
            },
            position(1),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(
            reconciliation.next_checkpoint.last_outcome,
            ReconcileOutcome::DuplicateStale
        );
        assert_eq!(reconciliation.next_checkpoint.status, current.status);
        assert_eq!(reconciliation.next_checkpoint.schedule, current.schedule);
    }

    #[test]
    fn schedule_change_exposes_its_schedule_id() {
        let event = ScheduleChange::Paused {
            schedule_id: schedule_id("orders"),
        };
        assert_eq!(event.schedule_id().as_str(), "orders");
    }
