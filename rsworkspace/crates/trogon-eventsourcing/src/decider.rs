mod act;
mod decide;
mod decision;
mod non_empty;

pub use act::Act;
pub use decide::Decide;
pub use decision::Decision;
pub use non_empty::NonEmpty;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct TestCommand;

    impl Decide for TestCommand {
        type StreamId = str;
        type State = u8;
        type Event = &'static str;
        type DecideError = ();
        type EvolveError = ();

        fn stream_id(&self) -> &Self::StreamId {
            "alpha"
        }

        fn initial_state() -> Self::State {
            0
        }

        fn evolve(state: Self::State, _event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
            Ok(state)
        }

        fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self::Event>, Self::DecideError> {
            Ok(Decision::event(if *state == 1 { "created" } else { "updated" }))
        }
    }

    #[test]
    fn act_is_constructible_and_defaultable() {
        let act = Act;
        let _ = act;
    }

    #[test]
    fn decide_exposes_typed_stream_id() {
        let command = TestCommand;
        assert_eq!(command.stream_id(), "alpha");
    }

    #[test]
    fn non_empty_rejects_empty_vectors() {
        assert!(NonEmpty::<u8>::from_vec(vec![]).is_none());
        assert_eq!(NonEmpty::from_vec(vec![1, 2]).unwrap().as_slice(), &[1, 2]);
    }

    #[test]
    fn non_empty_supports_map_and_iteration() {
        let events = NonEmpty::from_vec(vec![1, 2, 3]).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(events.map(|value| value.to_string()).into_vec(), vec!["1", "2", "3"]);
    }
}
