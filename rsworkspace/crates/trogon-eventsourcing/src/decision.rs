use crate::stream::StreamState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Act;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmpty<T>(Vec<T>);

impl<T> NonEmpty<T> {
    pub fn one(value: T) -> Self {
        Self(vec![value])
    }

    pub fn from_vec(values: Vec<T>) -> Option<Self> {
        if values.is_empty() { None } else { Some(Self(values)) }
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn first(&self) -> &T {
        &self.0[0]
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(&self) -> bool {
        false
    }

    pub fn map<U, F>(self, f: F) -> NonEmpty<U>
    where
        F: FnMut(T) -> U,
    {
        NonEmpty(self.0.into_iter().map(f).collect())
    }

    pub fn try_map<U, E, F>(self, mut f: F) -> Result<NonEmpty<U>, E>
    where
        F: FnMut(T) -> Result<U, E>,
    {
        let mut values = Vec::with_capacity(self.0.len());
        for value in self.0 {
            values.push(f(value)?);
        }
        Ok(NonEmpty(values))
    }
}

impl<T> IntoIterator for NonEmpty<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decision<E> {
    Event(NonEmpty<E>),
}

impl<E> Decision<E> {
    pub fn event(event: impl Into<E>) -> Self {
        Self::Event(NonEmpty::one(event.into()))
    }
}

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
