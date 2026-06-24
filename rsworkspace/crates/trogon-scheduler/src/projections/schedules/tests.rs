    use buffa::MessageField;
    use buffa_types::google::protobuf::{Duration, Timestamp};
    use chrono::DateTime;

    use super::*;
    use crate::v1;

    use projections_v1::__buffa::oneof::schedule_status::Kind as ViewStatusKind;

    fn timestamp_from_str(rfc3339: &str) -> Timestamp {
        let dt = DateTime::parse_from_rfc3339(rfc3339).unwrap();
        Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
            ..Default::default()
        }
    }

    fn proto_job_created(id: &str) -> v1::ScheduleCreated {
        v1::ScheduleCreated {
            schedule_id: id.to_string(),
            status: MessageField::some(v1::ScheduleStatus {
                kind: Some(v1::schedule_status::Scheduled {}.into()),
            }),
            schedule: MessageField::some(v1::Schedule {
                kind: Some(
                    v1::schedule::Every {
                        every: MessageField::some(Duration {
                            seconds: 30,
                            ..Default::default()
                        }),
                    }
                    .into(),
                ),
            }),
            delivery: MessageField::some(v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: "agent.run".to_string(),
                        ttl: MessageField::none(),
                        source: MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: MessageField::some(v1::Message {
                content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
                    content_type: "application/json".to_string(),
                    data: r#"{"kind":"heartbeat"}"#.as_bytes().to_vec(),
                }),
                headers: Vec::new(),
            }),
        }
    }

    fn added_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(proto_job_created(id).into()),
        }
    }

    fn paused_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn removed_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn completed_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCompleted {
                    schedule_id: id.to_string(),
                    last_occurrence_sequence: Some(2),
                }
                .into(),
            ),
        }
    }

    fn occurrence_scheduled_event(id: &str, at: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceScheduled {
                    schedule_id: id.to_string(),
                    occurrence_sequence: Some(1),
                    occurrence_at: MessageField::some(timestamp_from_str(at)),
                    scheduled_at: MessageField::some(timestamp_from_str(at)),
                }
                .into(),
            ),
        }
    }

    fn occurrence_recorded_event(id: &str, sequence: u64, at: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: id.to_string(),
                    occurrence_sequence: Some(sequence),
                    occurrence_at: MessageField::some(timestamp_from_str(at)),
                    recorded_at: MessageField::some(timestamp_from_str(at)),
                }
                .into(),
            ),
        }
    }

    fn present(state: ScheduleStreamState) -> projections_v1::ScheduleProjection {
        match state {
            ScheduleStreamState::Present(view) => view,
            other => panic!("expected present schedule, got {other:?}"),
        }
    }

    fn is_paused(view: &projections_v1::ScheduleProjection) -> bool {
        matches!(
            view.status.as_option().and_then(|status| status.kind.as_ref()),
            Some(ViewStatusKind::Paused(_))
        )
    }

    #[test]
    fn created_event_copies_the_event_definition_into_the_view() {
        let created = proto_job_created("backup");
        let view = present(apply("backup", initial_state(), &added_event("backup")).unwrap());

        assert_eq!(view.schedule_id, "backup");
        assert_eq!(view.completed, Some(false));
        assert!(view.next_occurrence_at.as_option().is_none());
        // The definition fields are folded field-for-field from the event into
        // the read model's own projection copies.
        assert_eq!(
            view.schedule.into_option(),
            created.schedule.into_option().map(twin::schedule_to_projection)
        );
        assert_eq!(
            view.delivery.into_option(),
            created.delivery.into_option().map(twin::delivery_to_projection)
        );
        assert_eq!(
            view.message.into_option(),
            created.message.into_option().map(twin::message_to_projection)
        );
    }

    #[test]
    fn pause_then_resume_toggles_status() {
        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let paused = present(apply("backup", created, &paused_event("backup")).unwrap());
        assert!(is_paused(&paused));

        let resumed = present(
            apply(
                "backup",
                ScheduleStreamState::Present(paused),
                &v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleResumed {
                            schedule_id: "backup".to_string(),
                        }
                        .into(),
                    ),
                },
            )
            .unwrap(),
        );
        assert!(!is_paused(&resumed));
    }

    #[test]
    fn pause_retains_and_resume_clears_the_pending_next_occurrence() {
        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let scheduled = present(
            apply(
                "backup",
                created,
                &occurrence_scheduled_event("backup", "2026-06-04T00:00:00+00:00"),
            )
            .unwrap(),
        );
        assert!(scheduled.next_occurrence_at.as_option().is_some());

        // Pause keeps the pending occurrence (durable progress retained while disabled).
        let paused = present(
            apply(
                "backup",
                ScheduleStreamState::Present(scheduled),
                &paused_event("backup"),
            )
            .unwrap(),
        );
        assert!(is_paused(&paused));
        assert!(
            paused.next_occurrence_at.as_option().is_some(),
            "pausing retains the pending occurrence"
        );

        // Resume discards the unrecorded paused wakeup so scheduling can re-arm.
        let resumed = present(
            apply(
                "backup",
                ScheduleStreamState::Present(paused),
                &v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleResumed {
                            schedule_id: "backup".to_string(),
                        }
                        .into(),
                    ),
                },
            )
            .unwrap(),
        );
        assert!(!is_paused(&resumed));
        assert!(
            resumed.next_occurrence_at.as_option().is_none(),
            "resuming clears the pending occurrence"
        );
    }

    #[test]
    fn event_projection_replays_latest_state() {
        let mut state = initial_state();
        for event in [added_event("backup"), paused_event("backup"), removed_event("backup")] {
            state = apply("backup", state, &event).unwrap();
        }
        assert_eq!(state, ScheduleStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn completed_event_marks_completed_without_removing() {
        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let view = present(apply("backup", created, &completed_event("backup")).unwrap());
        assert_eq!(view.completed, Some(true));
        assert!(view.next_occurrence_at.as_option().is_none());
    }

    #[test]
    fn tracks_next_and_last_occurrence() {
        let at = timestamp_from_str("2026-06-04T00:00:00+00:00");

        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let scheduled = present(
            apply(
                "backup",
                created,
                &occurrence_scheduled_event("backup", "2026-06-04T00:00:00+00:00"),
            )
            .unwrap(),
        );
        assert_eq!(scheduled.next_occurrence_at.as_option(), Some(&at));
        assert!(scheduled.last_occurrence_at.as_option().is_none());

        let recorded = present(
            apply(
                "backup",
                ScheduleStreamState::Present(scheduled),
                &occurrence_recorded_event("backup", 1, "2026-06-04T00:00:00+00:00"),
            )
            .unwrap(),
        );
        assert_eq!(recorded.last_occurrence_at.as_option(), Some(&at));
        assert!(
            recorded.next_occurrence_at.as_option().is_none(),
            "recording consumes the pending occurrence"
        );
    }

    #[test]
    fn rejects_recreating_a_deleted_schedule() {
        let error = apply(
            "backup",
            ScheduleStreamState::Deleted("backup".to_string()),
            &added_event("backup"),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::CannotAddDeletedSchedule { .. }
        ));
    }

    #[test]
    fn state_change_requires_an_existing_schedule() {
        let error = apply("missing", initial_state(), &paused_event("missing")).unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::MissingScheduleForStateChange { .. }
        ));
    }

    #[test]
    fn rejects_recreating_an_existing_schedule() {
        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let error = apply("backup", created, &added_event("backup")).unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::CannotAddExistingSchedule { .. }
        ));
    }

    #[test]
    fn initial_removal_creates_a_deleted_tombstone() {
        let state = apply("backup", initial_state(), &removed_event("backup")).unwrap();
        assert_eq!(state, ScheduleStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn mismatched_payload_id_is_rejected() {
        let error = apply("alpha", initial_state(), &added_event("beta")).unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::MismatchedEventScheduleId { .. }
        ));
    }

    #[test]
    fn projection_change_upserts_then_deletes() {
        let before = initial_state();
        let after = apply("backup", before.clone(), &added_event("backup")).unwrap();
        assert!(matches!(
            projection_change(&before, &after),
            Some(ProjectionChange::Upsert(_))
        ));

        let removed = apply("backup", after.clone(), &removed_event("backup")).unwrap();
        match projection_change(&after, &removed) {
            Some(ProjectionChange::Delete(id)) => assert_eq!(id, "backup"),
            other => panic!("expected delete, got {other:?}"),
        }
    }

    #[test]
    fn read_model_state_rejects_recreating_deleted_schedule() {
        let mut states = BTreeMap::new();
        let id = "alpha".to_string();
        apply_event_to_read_model_state(&mut states, &id, &added_event("alpha")).unwrap();
        apply_event_to_read_model_state(&mut states, &id, &removed_event("alpha")).unwrap();
        let error = apply_event_to_read_model_state(&mut states, &id, &added_event("alpha")).unwrap_err();
        assert_eq!(error.to_string(), "job 'alpha' was deleted and cannot be added again");
        assert_eq!(states.get("alpha"), Some(&ScheduleStreamState::Deleted(id)));
    }

    #[test]
    fn round_trips_through_the_kv_codec() {
        // What the projection writes must decode back to an equal view.
        let view = present(apply("backup", initial_state(), &added_event("backup")).unwrap());
        let encoded = buffa::Message::encode_to_vec(&view);
        let decoded = <projections_v1::ScheduleProjection as buffa::Message>::decode_from_slice(&encoded).unwrap();
        assert_eq!(decoded, view);
    }

