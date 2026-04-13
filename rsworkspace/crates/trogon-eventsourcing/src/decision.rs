#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Act;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmpty<T>(Vec<T>);

impl<T> NonEmpty<T> {
    pub fn one(value: T) -> Self {
        Self(vec![value])
    }

    pub fn from_vec(values: Vec<T>) -> Option<Self> {
        if values.is_empty() {
            None
        } else {
            Some(Self(values))
        }
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
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

pub trait Decide<State, Event> {
    type Error;

    fn decide(state: &State, command: &Self) -> Result<Decision<Event>, Self::Error>;
}

pub fn decide<State, Event, C>(state: &State, command: &C) -> Result<Decision<Event>, C::Error>
where
    C: Decide<State, Event>,
{
    C::decide(state, command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct TestCommand;

    impl Decide<u8, &'static str> for TestCommand {
        type Error = ();

        fn decide(state: &u8, _command: &Self) -> Result<Decision<&'static str>, Self::Error> {
            Ok(Decision::Event(NonEmpty::one(if *state == 1 {
                "created"
            } else {
                "updated"
            })))
        }
    }

    #[test]
    fn act_is_constructible_and_defaultable() {
        let act = Act;
        let _ = act;
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
        assert_eq!(
            events.map(|value| value.to_string()).into_vec(),
            vec!["1", "2", "3"]
        );
    }

    #[test]
    fn decide_dispatches_to_trait_implementation() {
        let decision = decide(&1, &TestCommand).unwrap();
        assert_eq!(decision, Decision::Event(NonEmpty::one("created")));
    }
}
