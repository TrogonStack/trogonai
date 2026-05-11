use super::NonEmpty;

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
