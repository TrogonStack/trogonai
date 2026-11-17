use crate::Evolve;

/// State initialization and event replay shared by deciders.
///
/// Implemented automatically for types implementing [`Default`] and [`Evolve`].
pub trait DeciderState<Event>: Default + Evolve<Event> {}

impl<T, Event> DeciderState<Event> for T where T: Default + Evolve<Event> {}
