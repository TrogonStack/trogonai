use super::Decision;
use crate::stream::StreamState;

pub trait Decide: Sized {
    type StreamId: ?Sized;
    type State;
    type Event;
    type DecideError;
    type EvolveError;

    const REQUIRED_WRITE_PRECONDITION: Option<StreamState> = None;

    fn stream_id(&self) -> &Self::StreamId;

    fn initial_state() -> Self::State;

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError>;

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self::Event>, Self::DecideError>;
}
