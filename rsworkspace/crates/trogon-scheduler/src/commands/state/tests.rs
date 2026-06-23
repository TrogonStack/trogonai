use buffa::MessageField;
    use trogonai_proto::scheduler::schedules::v1;

    use super::*;

    fn state(value: state_v1::StateValue) -> state_v1::State {
        state_v1::State {
            completed: None,
            state: Some(EnumValue::from(value)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        }
    }

    fn created(status: v1::ScheduleStatus) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: "backup".to_string(),
                    status: MessageField::some(status),
                    schedule: MessageField::default(),
                    delivery: MessageField::default(),
                    message: MessageField::default(),
                }
                .into(),
            ),
        }
    }

    fn paused() -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: "backup".to_string(),
                }
                .into(),
            ),
        }
    }

    fn resumed() -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleResumed {
                    schedule_id: "backup".to_string(),
                }
                .into(),
            ),
        }
    }

    fn occurrence_recorded() -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: "backup".to_string(),
                    occurrence_sequence: Some(1),
                    occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                        &occurrence_at(),
                    )),
                    recorded_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at())),
                }
                .into(),
            ),
        }
    }

    fn occurrence_scheduled() -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceScheduled {
                    schedule_id: "backup".to_string(),
                    occurrence_sequence: Some(2),
                    occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                        &occurrence_at(),
                    )),
                    scheduled_at: MessageField::some(
                        trogonai_proto::convert::timestamp_from_datetime(&occurrence_at()),
                    ),
                }
                .into(),
            ),
        }
    }

    fn completed() -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCompleted {
                    schedule_id: "backup".to_string(),
                    last_occurrence_sequence: Some(1),
                }
                .into(),
            ),
        }
    }

    fn occurrence_at() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-06-15T18:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn initial_state_is_missing() {
        assert_eq!(
            initial_state().state.unwrap().as_known(),
            Some(state_v1::StateValue::STATE_VALUE_MISSING)
        );
    }

    #[test]
    fn evolve_created_tracks_enabled_and_disabled_status() {
        let enabled = v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Scheduled {}.into()),
        };
        let disabled = v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Paused {}.into()),
        };

        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_MISSING), &created(enabled))
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
        );
        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_MISSING), &created(disabled))
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)
        );
    }

    #[test]
    fn evolve_lifecycle_events_track_enabled_and_disabled_status() {
        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED), &paused())
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)
        );
        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED), &resumed())
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
        );
        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_MISSING), &paused())
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)
        );
        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_MISSING), &resumed())
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
        );
    }

    #[test]
    fn evolve_removed_and_deleted_state_are_terminal() {
        let removed = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: "backup".to_string(),
                }
                .into(),
            ),
        };
        let created = created(v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Scheduled {}.into()),
        });

        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED), &removed)
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_DELETED)
        );
        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_DELETED), &created)
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_DELETED)
        );
        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_DELETED), &paused())
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_DELETED)
        );
        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_DELETED), &resumed())
                .unwrap()
                .state
                .unwrap()
                .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_DELETED)
        );
    }

    #[test]
    fn evolve_reports_invalid_state_and_event_shapes() {
        assert_eq!(
            evolve(
                state_v1::State {
                    completed: None,
                    state: None,
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &v1::ScheduleEvent { event: None }
            )
            .unwrap_err(),
            EvolveError::MissingStateValue
        );
        assert_eq!(
            evolve(
                state(state_v1::StateValue::STATE_VALUE_UNSPECIFIED),
                &v1::ScheduleEvent { event: None }
            )
            .unwrap_err(),
            EvolveError::UnknownStateValue { value: 0 }
        );
        assert_eq!(
            evolve(
                state(state_v1::StateValue::STATE_VALUE_MISSING),
                &v1::ScheduleEvent { event: None }
            )
            .unwrap_err(),
            EvolveError::UnsupportedEvent
        );
        assert_eq!(
            EvolveError::UnsupportedEvent.to_string(),
            "protobuf schedule event is not supported by command state"
        );
        assert_eq!(
            EvolveError::MissingStateValue.to_string(),
            "protobuf state is missing its state value"
        );
        assert_eq!(
            EvolveError::UnknownStateValue { value: 123 }.to_string(),
            "protobuf state '123' is unknown"
        );
        assert_eq!(
            evolve(
                state_v1::State {
                    completed: None,
                    state: Some(EnumValue::from(123)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &v1::ScheduleEvent { event: None }
            )
            .unwrap_err(),
            EvolveError::UnknownStateValue { value: 123 }
        );
    }

    #[test]
    fn evolve_created_resets_progress_and_stashes_schedule() {
        let dirty = state_v1::State {
            completed: Some(true),
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at())),
            last_occurrence_sequence: Some(9),
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                &occurrence_at(),
            )),
        };
        let schedule = v1::Schedule {
            kind: Some(
                v1::schedule::Every {
                    every: MessageField::some(buffa_types::google::protobuf::Duration {
                        seconds: 30,
                        nanos: 0,
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        };
        let created = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: "backup".to_string(),
                    status: MessageField::some(v1::ScheduleStatus {
                        kind: Some(v1::schedule_status::Scheduled {}.into()),
                    }),
                    schedule: MessageField::some(schedule.clone()),
                    delivery: MessageField::default(),
                    message: MessageField::default(),
                }
                .into(),
            ),
        };

        let next = evolve(dirty, &created).unwrap();

        assert_eq!(next.last_occurrence_at.as_option(), None);
        assert_eq!(next.last_occurrence_sequence, None);
        assert_eq!(next.pending_occurrence_at.as_option(), None);
        assert_eq!(next.schedule.as_option(), Some(&schedule));
        assert_eq!(next.completed, None);
    }

    #[test]
    fn evolve_occurrence_recorded_advances_progress_and_clears_pending() {
        let event = occurrence_recorded();
        let pending = state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                &occurrence_at(),
            )),
        };
        let next = evolve(pending, &event).unwrap();
        let expected = trogonai_proto::convert::timestamp_from_datetime(&occurrence_at());

        assert_eq!(
            next.state.unwrap().as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
        );
        assert_eq!(next.last_occurrence_at.as_option(), Some(&expected));
        assert_eq!(next.last_occurrence_sequence, Some(1));
        assert_eq!(next.pending_occurrence_at.as_option(), None);
    }

    #[test]
    fn evolve_occurrence_scheduled_sets_pending_without_changing_presence() {
        let next = evolve(
            state(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED),
            &occurrence_scheduled(),
        )
        .unwrap();
        let expected = trogonai_proto::convert::timestamp_from_datetime(&occurrence_at());

        assert_eq!(
            next.state.unwrap().as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
        );
        assert_eq!(next.pending_occurrence_at.as_option(), Some(&expected));
        assert_eq!(next.last_occurrence_sequence, None);
    }

    #[test]
    fn evolve_completed_clears_pending() {
        let pending = state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at())),
            last_occurrence_sequence: Some(1),
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                &occurrence_at(),
            )),
        };

        let next = evolve(pending, &completed()).unwrap();

        assert_eq!(next.pending_occurrence_at.as_option(), None);
        assert_eq!(next.last_occurrence_sequence, Some(1));
        assert_eq!(next.completed, Some(true));
        assert_eq!(
            next.state.unwrap().as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
        );
    }

    #[test]
    fn evolve_paused_retains_pending_so_in_flight_recording_can_land() {
        let pending = state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                &occurrence_at(),
            )),
        };

        let next = evolve(pending, &paused()).unwrap();
        let expected = trogonai_proto::convert::timestamp_from_datetime(&occurrence_at());

        assert_eq!(
            next.state.unwrap().as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)
        );
        assert_eq!(next.pending_occurrence_at.as_option(), Some(&expected));
    }

    #[test]
    fn evolve_resumed_clears_unrecorded_paused_pending_so_schedule_can_rearm() {
        let pending = state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                &occurrence_at(),
            )),
        };

        let next = evolve(pending, &resumed()).unwrap();

        assert_eq!(
            next.state.unwrap().as_known(),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
        );
        assert_eq!(next.pending_occurrence_at.as_option(), None);
    }

    #[test]
    fn evolve_occurrence_recorded_requires_occurrence_at() {
        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: "backup".to_string(),
                    occurrence_sequence: Some(1),
                    occurrence_at: MessageField::default(),
                    recorded_at: MessageField::default(),
                }
                .into(),
            ),
        };

        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED), &event).unwrap_err(),
            EvolveError::MissingOccurrenceAt
        );
    }

    #[test]
    fn evolve_occurrence_recorded_requires_occurrence_sequence() {
        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: "backup".to_string(),
                    occurrence_sequence: None,
                    occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                        &occurrence_at(),
                    )),
                    recorded_at: MessageField::default(),
                }
                .into(),
            ),
        };

        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED), &event).unwrap_err(),
            EvolveError::MissingOccurrenceSequence
        );
    }

    #[test]
    fn evolve_occurrence_scheduled_requires_occurrence_at() {
        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceScheduled {
                    schedule_id: "backup".to_string(),
                    occurrence_sequence: Some(2),
                    occurrence_at: MessageField::default(),
                    scheduled_at: MessageField::default(),
                }
                .into(),
            ),
        };

        assert_eq!(
            evolve(state(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED), &event).unwrap_err(),
            EvolveError::MissingOccurrenceAt
        );
    }
