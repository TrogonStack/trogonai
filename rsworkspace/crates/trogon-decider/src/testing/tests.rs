use std::panic::{AssertUnwindSafe, catch_unwind};

use super::*;
use crate::Decision;

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestState {
    Missing,
    Present { enabled: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestEvent {
    Registered { id: String },
    Disabled { id: String },
    Removed { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum TestDomainError {
    #[error("{self:?}")]
    MissingJobForDisable { id: String },
    #[error("{self:?}")]
    MissingJobForRemoval { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum TestCommandError {
    #[error("{self:?}")]
    AlreadyRegistered { id: String },
    #[error("{self:?}")]
    JobNotFound { id: String },
    #[error("{self:?}")]
    AlreadyDisabled { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestAction {
    Register,
    Disable,
    Remove,
    EmitDisabled,
    EmitRegistered,
    ActReject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestCommand {
    id: &'static str,
    action: TestAction,
}

impl TestCommand {
    fn register(id: &'static str) -> Self {
        Self {
            id,
            action: TestAction::Register,
        }
    }

    fn disable(id: &'static str) -> Self {
        Self {
            id,
            action: TestAction::Disable,
        }
    }

    fn remove(id: &'static str) -> Self {
        Self {
            id,
            action: TestAction::Remove,
        }
    }

    fn emit_disabled(id: &'static str) -> Self {
        Self {
            id,
            action: TestAction::EmitDisabled,
        }
    }

    fn emit_registered(id: &'static str) -> Self {
        Self {
            id,
            action: TestAction::EmitRegistered,
        }
    }

    fn act_reject(id: &'static str) -> Self {
        Self {
            id,
            action: TestAction::ActReject,
        }
    }
}

impl Decider for TestCommand {
    type StreamId = str;
    type State = TestState;
    type Event = TestEvent;
    type DecideError = TestCommandError;
    type EvolveError = TestDomainError;

    fn stream_id(&self) -> &Self::StreamId {
        self.id
    }

    fn initial_state() -> Self::State {
        TestState::Missing
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        match (state, event) {
            (TestState::Missing, TestEvent::Registered { .. }) => Ok(TestState::Present { enabled: true }),
            (TestState::Missing, TestEvent::Disabled { id }) => {
                Err(TestDomainError::MissingJobForDisable { id: id.clone() })
            }
            (TestState::Missing, TestEvent::Removed { id }) => {
                Err(TestDomainError::MissingJobForRemoval { id: id.clone() })
            }
            (TestState::Present { .. }, TestEvent::Registered { id }) => {
                Err(TestDomainError::MissingJobForDisable { id: id.clone() })
            }
            (TestState::Present { .. }, TestEvent::Disabled { .. }) => Ok(TestState::Present { enabled: false }),
            (TestState::Present { .. }, TestEvent::Removed { .. }) => Ok(TestState::Missing),
        }
    }

    fn decide(state: &TestState, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        match (&command.action, state) {
            (TestAction::Register, TestState::Missing) => Ok(Decision::event(TestEvent::Registered {
                id: command.id.to_string(),
            })),
            (TestAction::Register, TestState::Present { .. }) => Err(TestCommandError::AlreadyRegistered {
                id: command.id.to_string(),
            }),
            (TestAction::Disable, TestState::Missing) => Err(TestCommandError::JobNotFound {
                id: command.id.to_string(),
            }),
            (TestAction::Disable, TestState::Present { enabled: false }) => Err(TestCommandError::AlreadyDisabled {
                id: command.id.to_string(),
            }),
            (TestAction::Disable, TestState::Present { enabled: true }) => Ok(Decision::event(TestEvent::Disabled {
                id: command.id.to_string(),
            })),
            (TestAction::Remove, TestState::Missing) => Err(TestCommandError::JobNotFound {
                id: command.id.to_string(),
            }),
            (TestAction::Remove, TestState::Present { .. }) => Ok(Decision::event(TestEvent::Removed {
                id: command.id.to_string(),
            })),
            (TestAction::EmitDisabled, _) => Ok(Decision::event(TestEvent::Disabled {
                id: command.id.to_string(),
            })),
            (TestAction::EmitRegistered, _) => Ok(Decision::event(TestEvent::Registered {
                id: command.id.to_string(),
            })),
            (TestAction::ActReject, _) => Decision::act()
                .execute(|_: &TestState, command: &TestCommand| {
                    Err(TestCommandError::JobNotFound {
                        id: command.id.to_string(),
                    })
                })
                .into(),
        }
    }

    fn decide_error_code(error: &Self::DecideError) -> &str {
        match error {
            TestCommandError::AlreadyRegistered { .. } => "already-registered",
            TestCommandError::JobNotFound { .. } => "job-not-found",
            TestCommandError::AlreadyDisabled { .. } => "already-disabled",
        }
    }
}

#[test]
fn given_empty_history_when_command_then_array_events_succeeds() {
    let register = TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::register("alpha"))
        .then([TestEvent::Registered {
            id: "alpha".to_string(),
        }]);

    assert_eq!(register.stream_id(), "alpha");
    assert_eq!(register.given_events(), []);
    assert_eq!(
        register.then_events(),
        [TestEvent::Registered {
            id: "alpha".to_string(),
        }]
    );
}

#[test]
fn given_history_when_command_then_array_events_rehydrates_state() {
    TestCase::<TestCommand>::new()
        .given([TestEvent::Registered {
            id: "alpha".to_string(),
        }])
        .when(TestCommand::disable("alpha"))
        .then([TestEvent::Disabled {
            id: "alpha".to_string(),
        }]);
}

#[test]
fn given_history_when_command_then_exact_error_succeeds() {
    TestCase::<TestCommand>::new()
        .given([TestEvent::Registered {
            id: "alpha".to_string(),
        }])
        .when(TestCommand::register("alpha"))
        .then_error(TestCommandError::AlreadyRegistered {
            id: "alpha".to_string(),
        });
}

#[test]
fn state_can_assert_replayed_state_before_events() {
    TestCase::<TestCommand>::new()
        .given([TestEvent::Registered {
            id: "alpha".to_string(),
        }])
        .when(TestCommand::disable("alpha"))
        .state(TestState::Present { enabled: true })
        .then([TestEvent::Disabled {
            id: "alpha".to_string(),
        }]);
}

#[test]
fn state_can_assert_replayed_state_before_errors() {
    TestCase::<TestCommand>::new()
        .given([TestEvent::Registered {
            id: "alpha".to_string(),
        }])
        .given([TestEvent::Disabled {
            id: "alpha".to_string(),
        }])
        .when(TestCommand::disable("alpha"))
        .state(TestState::Present { enabled: false })
        .then_error(TestCommandError::AlreadyDisabled {
            id: "alpha".to_string(),
        });
}

#[test]
#[should_panic(expected = "Given history could not be replayed at event 1")]
fn invalid_history_in_given_panics_during_replay() {
    let _ = TestCase::<TestCommand>::new()
        .given([TestEvent::Removed {
            id: "alpha".to_string(),
        }])
        .when(TestCommand::register("alpha"));
}

#[test]
fn then_vec_events_works() {
    TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::register("alpha"))
        .then(vec![TestEvent::Registered {
            id: "alpha".to_string(),
        }]);
}

#[test]
fn then_events_works() {
    TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::register("alpha"))
        .then(Events::one(TestEvent::Registered {
            id: "alpha".to_string(),
        }));
}

#[test]
fn then_error_works() {
    let error = TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::remove("alpha"))
        .then_error(TestCommandError::JobNotFound {
            id: "alpha".to_string(),
        });

    assert_eq!(error.stream_id(), "alpha");
    assert_eq!(error.given_events(), []);
    assert_eq!(
        error.error(),
        &TestCommandError::JobNotFound {
            id: "alpha".to_string(),
        }
    );
}

#[test]
fn then_error_works_for_disable_without_history() {
    TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::disable("alpha"))
        .then_error(TestCommandError::JobNotFound {
            id: "alpha".to_string(),
        });
}

#[test]
fn test_errors_display_debug_shape() {
    assert_eq!(
        TestDomainError::MissingJobForDisable {
            id: "alpha".to_string()
        }
        .to_string(),
        r#"MissingJobForDisable { id: "alpha" }"#
    );
    assert_eq!(
        TestCommandError::AlreadyRegistered {
            id: "alpha".to_string()
        }
        .to_string(),
        r#"AlreadyRegistered { id: "alpha" }"#
    );
}

#[test]
fn history_combines_given_and_emitted_events() {
    let register = TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::register("alpha"))
        .then([TestEvent::Registered {
            id: "alpha".to_string(),
        }]);

    let disable = TestCase::<TestCommand>::new()
        .given(register.history())
        .when(TestCommand::disable("alpha"))
        .then([TestEvent::Disabled {
            id: "alpha".to_string(),
        }]);

    assert_eq!(
        disable.history().as_slice(),
        [
            TestEvent::Registered {
                id: "alpha".to_string(),
            },
            TestEvent::Disabled {
                id: "alpha".to_string(),
            },
        ]
    );
}

#[test]
#[should_panic(expected = "event expectations must be non-empty")]
fn then_empty_event_vec_panics() {
    TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::register("alpha"))
        .then(Vec::<TestEvent>::new());
}

#[test]
fn multiple_given_calls_append_to_history() {
    TestCase::<TestCommand>::new()
        .given([TestEvent::Registered {
            id: "alpha".to_string(),
        }])
        .given([TestEvent::Disabled {
            id: "alpha".to_string(),
        }])
        .when(TestCommand::remove("alpha"))
        .then([TestEvent::Removed {
            id: "alpha".to_string(),
        }]);
}

#[test]
fn history_conversions_and_accessors_work() {
    let empty = History::<TestEvent>::default();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);

    let from_vec = History::from(vec![TestEvent::Registered {
        id: "alpha".to_string(),
    }]);
    assert_eq!(from_vec.as_ref(), from_vec.as_slice());
    assert_eq!(from_vec.iter().count(), 1);

    let from_array = History::from([TestEvent::Disabled {
        id: "alpha".to_string(),
    }]);
    assert_eq!(from_array.into_vec().len(), 1);

    let collected = [
        TestEvent::Registered {
            id: "alpha".to_string(),
        },
        TestEvent::Disabled {
            id: "alpha".to_string(),
        },
    ]
    .into_iter()
    .collect::<History<_>>();
    assert_eq!(collected.into_iter().count(), 2);
}

#[test]
fn default_test_case_starts_a_valid_chain() {
    TestCase::<TestCommand>::default()
        .given_no_history()
        .when(TestCommand::register("alpha"))
        .then([TestEvent::Registered {
            id: "alpha".to_string(),
        }]);
}

#[test]
fn unfinished_test_case_panics_on_drop() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _case = TestCase::<TestCommand>::new().given_no_history();
    }))
    .unwrap_err();

    let message = panic_message(&panic);
    assert!(message.contains("test cases must be completed with .then(...) or .then_error(...)"));
}

#[test]
fn event_mismatch_shows_expected_vs_actual() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::register("alpha"))
            .then([TestEvent::Removed {
                id: "alpha".to_string(),
            }]);
    }))
    .unwrap_err();

    let message = panic_message(&panic);
    assert!(message.contains("then(...) emitted events did not match expectation"));
    assert!(message.contains("Registered"));
    assert!(message.contains("Removed"));
}

#[test]
fn event_expectation_panics_when_decide_rejects() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::remove("alpha"))
            .then([TestEvent::Removed {
                id: "alpha".to_string(),
            }]);
    }))
    .unwrap_err();

    let message = panic_message(&panic);
    assert!(message.contains("then(...) expected events"));
    assert!(message.contains("JobNotFound"));
}

#[test]
fn event_expectation_panics_when_emitted_events_do_not_evolve() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::emit_disabled("alpha"))
            .then([TestEvent::Disabled {
                id: "alpha".to_string(),
            }]);
    }))
    .unwrap_err();

    let message = panic_message(&panic);
    assert!(message.contains("emitted events could not be evolved"));
    assert!(message.contains("MissingJobForDisable"));
}

#[test]
fn error_expectation_panics_when_emitted_events_do_not_evolve() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::emit_disabled("alpha"))
            .then_error(TestCommandError::AlreadyDisabled {
                id: "alpha".to_string(),
            });
    }))
    .unwrap_err();

    let message = panic_message(&panic);
    assert!(message.contains("then_error(...) expected a decide error"));
    assert!(message.contains("MissingJobForDisable"));
}

#[test]
fn act_decide_failure_is_reported_as_decide_error() {
    TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::act_reject("alpha"))
        .then_error(TestCommandError::JobNotFound {
            id: "alpha".to_string(),
        });
}

#[test]
fn invalid_emitted_event_after_history_reports_evolve_error() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        TestCase::<TestCommand>::new()
            .given([TestEvent::Registered {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::emit_registered("alpha"))
            .then([TestEvent::Registered {
                id: "alpha".to_string(),
            }]);
    }))
    .unwrap_err();

    let message = panic_message(&panic);
    assert!(message.contains("MissingJobForDisable"));
}

#[test]
fn error_mismatch_shows_expected_vs_actual() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::register("alpha"))
            .then_error(TestCommandError::JobNotFound {
                id: "alpha".to_string(),
            });
    }))
    .unwrap_err();

    let message = panic_message(&panic);
    assert!(message.contains("then_error(...) expected an error"));
    assert!(message.contains("JobNotFound"));
    assert!(message.contains("Registered"));
}

#[test]
fn then_state_exposes_and_pins_the_resulting_state() {
    let registered = TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::register("alpha"))
        .then([TestEvent::Registered {
            id: "alpha".to_string(),
        }])
        .then_state(TestState::Present { enabled: true });

    assert_eq!(registered.resulting_state(), &TestState::Present { enabled: true });
}

#[test]
#[should_panic(expected = "then_state(...) resulting state did not match expectation")]
fn then_state_mismatch_panics() {
    TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::register("alpha"))
        .then([TestEvent::Registered {
            id: "alpha".to_string(),
        }])
        .then_state(TestState::Missing);
}

#[test]
fn then_error_code_exposes_and_pins_the_wire_code() {
    let error = TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::remove("alpha"))
        .then_error(TestCommandError::JobNotFound {
            id: "alpha".to_string(),
        })
        .then_error_code("job-not-found");

    assert_eq!(error.code(), "job-not-found");
}

#[test]
#[should_panic(expected = "then_error_code(...) expected decide_error_code")]
fn then_error_code_mismatch_panics() {
    TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand::remove("alpha"))
        .then_error(TestCommandError::JobNotFound {
            id: "alpha".to_string(),
        })
        .then_error_code("wrong-code");
}

#[test]
fn panic_message_returns_empty_for_unknown_payload() {
    let panic = catch_unwind(AssertUnwindSafe(|| std::panic::panic_any(123_u8))).unwrap_err();

    assert_eq!(panic_message(&panic), "");
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        return (*message).to_string();
    }
    String::new()
}
