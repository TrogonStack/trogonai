use super::*;

#[derive(Debug, PartialEq, Eq)]
struct TestCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum TestError {
    #[error("test decide error")]
    Decide,
    #[error("test evolve error")]
    Evolve,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TestState(u8);

impl Evolve<&'static str> for TestState {
    type Error = TestError;

    fn evolve(self, event: &&'static str) -> Result<Self, Self::Error> {
        if *event == "broken" {
            Err(TestError::Evolve)
        } else {
            Ok(Self(self.0 + 1))
        }
    }
}

impl Decider for TestCommand {
    type StreamId = str;
    type State = TestState;
    type Event = &'static str;
    type DecideError = TestError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }

    fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::event(if state.0 == 1 { "created" } else { "updated" }))
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PublishState {
    created: bool,
}

impl Evolve<&'static str> for PublishState {
    type Error = TestError;

    fn evolve(self, event: &&'static str) -> Result<Self, Self::Error> {
        Ok(Self {
            created: self.created || *event == "created",
        })
    }
}

struct PublishCommand;

impl Decider for PublishCommand {
    type StreamId = str;
    type State = PublishState;
    type Event = &'static str;
    type DecideError = TestError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }

    fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        if !state.created {
            return Err(TestError::Decide);
        }
        Ok(Decision::event("published"))
    }
}

#[test]
fn deciders_select_independent_state_projections_from_the_same_events() {
    let history = ["created", "updated"];
    let count = history.iter().try_fold(TestState::default(), Evolve::evolve).unwrap();
    let publish = history
        .iter()
        .try_fold(PublishState::default(), Evolve::evolve)
        .unwrap();

    assert_eq!(count, TestState(2));
    assert_eq!(publish, PublishState { created: true });
    assert_eq!(
        evaluate_decision(count, &TestCommand).unwrap().1.as_slice(),
        &["updated"]
    );
    assert_eq!(
        evaluate_decision(publish, &PublishCommand).unwrap().1.as_slice(),
        &["published"]
    );
}

#[test]
fn act_chains_plain_decisions() {
    let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
        .execute(|_, _| Decision::event("created"))
        .execute(|_, _| Decision::event("updated"))
        .into();

    let (state, events) = decision.unwrap().handle(TestState::default(), &TestCommand).unwrap();

    assert_eq!(state, TestState(2));
    assert_eq!(events.as_slice(), &["created", "updated"]);
}

#[test]
fn act_accepts_result_steps() {
    let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
        .execute(|_, _| Ok(Decision::event("created")))
        .into();

    let (state, events) = decision.unwrap().handle(TestState::default(), &TestCommand).unwrap();

    assert_eq!(state, TestState(1));
    assert_eq!(events.as_slice(), &["created"]);
}

#[test]
fn act_steps_see_evolved_state() {
    let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
        .execute(|_, _| Decision::event("created"))
        .execute(|state, _| Decision::event(if state.0 == 1 { "updated" } else { "broken" }))
        .into();

    let (state, events) = decision.unwrap().handle(TestState::default(), &TestCommand).unwrap();

    assert_eq!(state, TestState(2));
    assert_eq!(events.as_slice(), &["created", "updated"]);
}

#[test]
fn act_propagates_decide_errors() {
    let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
        .execute(|_, _| Err(TestError::Decide))
        .into();

    assert_eq!(
        decision.unwrap().handle(TestState::default(), &TestCommand),
        Err(DecisionError::Decide(TestError::Decide))
    );
}

#[test]
fn act_propagates_evolve_errors() {
    let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
        .execute(|_, _| Decision::event("broken"))
        .into();

    assert_eq!(
        decision.unwrap().handle(TestState::default(), &TestCommand),
        Err(DecisionError::Evolve(TestError::Evolve))
    );
}

#[test]
fn decide_exposes_typed_stream_id() {
    let command = TestCommand;
    assert_eq!(command.stream_id(), "alpha");
}

#[test]
fn decider_trait_exposes_initial_state_and_decision() {
    let command = TestCommand;
    let (state, events) = evaluate_decision::<TestCommand>(TestState::default(), &command).unwrap();

    assert_eq!(state, TestState(1));
    assert_eq!(events.as_slice(), &["updated"]);
}

#[test]
fn events_rejects_empty_vectors() {
    assert!(Events::<u8>::from_vec(vec![]).is_none());
    assert_eq!(Events::from_vec(vec![1, 2]).unwrap().as_slice(), &[1, 2]);
}

#[test]
fn events_supports_map_and_iteration() {
    let events = Events::from_vec(vec![1, 2, 3]).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(*events.first(), 1);
    assert!(!events.is_empty());
    assert_eq!(events.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(events.map(|value| value.to_string()).into_vec(), vec!["1", "2", "3"]);
}

#[test]
fn events_supports_guaranteed_first_and_fallible_map() {
    let events = Events::from_first(1, vec![2, 3]);
    let mapped: Result<Events<_>, TestError> = events.clone().try_map(|value| Ok(value.to_string()));
    let failed: Result<Events<_>, TestError> =
        events.try_map(|value| if value == 2 { Err(TestError::Evolve) } else { Ok(value) });

    assert_eq!(mapped.unwrap().into_vec(), vec!["1", "2", "3"]);
    assert_eq!(failed, Err(TestError::Evolve));
}

#[test]
fn decision_event_evolves_state_through_handle() {
    let decision = Decision::<TestCommand>::event("created");
    let (state, events) = decision.handle(TestState::default(), &TestCommand).unwrap();
    assert_eq!(state, TestState(1));
    assert_eq!(events.as_slice(), &["created"]);
}

#[test]
fn decision_events_evolves_each_event_through_handle() {
    let payload = Events::from_vec(vec!["a", "b", "c"]).unwrap();
    let (state, events) = Decision::<TestCommand>::events(payload)
        .handle(TestState::default(), &TestCommand)
        .unwrap();
    assert_eq!(state, TestState(3));
    assert_eq!(events.as_slice(), &["a", "b", "c"]);
}

#[test]
fn decision_events_direct_path_propagates_evolve_errors() {
    let decision = Decision::<TestCommand>::event("broken");
    assert_eq!(
        decision.handle(TestState::default(), &TestCommand),
        Err(DecisionError::Evolve(TestError::Evolve))
    );
}

#[test]
fn decision_failure_exposes_display_and_source() {
    let decide = DecisionError::<TestError, TestError>::Decide(TestError::Decide);
    let evolve = DecisionError::<TestError, TestError>::Evolve(TestError::Evolve);

    assert_eq!(decide.to_string(), "decide failed: test decide error");
    assert_eq!(evolve.to_string(), "evolve failed: test evolve error");
    assert!(std::error::Error::source(&decide).is_some());
    assert!(std::error::Error::source(&evolve).is_some());
}

#[test]
fn act_chains_three_steps_threads_state() {
    let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
        .execute(|state, _| Decision::event(if state.0 == 0 { "a" } else { "broken" }))
        .execute(|state, _| Decision::event(if state.0 == 1 { "b" } else { "broken" }))
        .execute(|state, _| Decision::event(if state.0 == 2 { "c" } else { "broken" }))
        .into();

    let (state, events) = decision.unwrap().handle(TestState::default(), &TestCommand).unwrap();
    assert_eq!(state, TestState(3));
    assert_eq!(events.as_slice(), &["a", "b", "c"]);
}

#[test]
fn from_decision_to_result_wraps_in_ok() {
    let decision = Decision::<TestCommand>::event("created");
    let result: Result<Decision<TestCommand>, TestError> = decision.into();
    let (state, events) = result.unwrap().handle(TestState::default(), &TestCommand).unwrap();
    assert_eq!(state, TestState(1));
    assert_eq!(events.as_slice(), &["created"]);
}

#[test]
fn act_types_expose_debug_and_default() {
    type Step = fn(&TestState, &TestCommand) -> Decision<TestCommand>;

    let step: Step = |_, _| Decision::event("created");
    let chain = ActBuilder::<TestCommand>::default().execute(step);
    assert!(format!("{chain:?}").contains("ActChain"));

    let chain = chain.execute(step);
    let decision: Result<Decision<TestCommand>, TestError> = chain.into();
    let decision = decision.unwrap();

    assert!(format!("{:?}", ActBuilder::<TestCommand>::default()).contains("ActBuilder"));
    assert!(format!("{decision:?}").contains("Act"));
}
