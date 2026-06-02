use buffa::EnumValue;
use trogonai_proto::scheduler::schedules::{ScheduleEventCase, ScheduleStatusKind, state_v1, v1};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolveError {
    MissingStateValue,
    UnsupportedEvent,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for EvolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingStateValue => f.write_str("protobuf state is missing its state value"),
            Self::UnsupportedEvent => f.write_str("protobuf schedule event is not supported by command state"),
            Self::UnknownStateValue { value } => write!(f, "protobuf state '{value}' is unknown"),
        }
    }
}

impl std::error::Error for EvolveError {}

pub(super) fn initial_state() -> state_v1::State {
    state_v1::State {
        state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_MISSING)),
    }
}

pub(super) fn evolve(state: state_v1::State, event: &v1::ScheduleEvent) -> Result<state_v1::State, EvolveError> {
    let Some(value) = state.state.as_ref() else {
        return Err(EvolveError::MissingStateValue);
    };
    let Some(current_state) = value.as_known() else {
        return Err(EvolveError::UnknownStateValue { value: value.to_i32() });
    };
    let current_state = match current_state {
        state_v1::StateValue::STATE_VALUE_MISSING
        | state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
        | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
        | state_v1::StateValue::STATE_VALUE_DELETED => current_state,
        state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
            return Err(EvolveError::UnknownStateValue {
                value: current_state as i32,
            });
        }
    };
    let next_state = match &event.event {
        Some(ScheduleEventCase::ScheduleCreated(inner)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else if matches!(
                inner.status.as_option().and_then(|status| status.kind.as_ref()),
                Some(ScheduleStatusKind::Paused(_))
            ) {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        Some(ScheduleEventCase::ScheduleRemoved(_)) => state_v1::StateValue::STATE_VALUE_DELETED,
        Some(ScheduleEventCase::SchedulePaused(_)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            }
        }
        Some(ScheduleEventCase::ScheduleResumed(_)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        None => return Err(EvolveError::UnsupportedEvent),
    };

    Ok(state_v1::State {
        state: Some(EnumValue::from(next_state)),
    })
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;
    use trogonai_proto::scheduler::schedules::v1;

    use super::*;

    fn state(value: state_v1::StateValue) -> state_v1::State {
        state_v1::State {
            state: Some(EnumValue::from(value)),
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
            evolve(state_v1::State { state: None }, &v1::ScheduleEvent { event: None }).unwrap_err(),
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
                    state: Some(EnumValue::from(123)),
                },
                &v1::ScheduleEvent { event: None }
            )
            .unwrap_err(),
            EvolveError::UnknownStateValue { value: 123 }
        );
    }
}
