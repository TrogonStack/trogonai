use crate::{Decider, Events};

pub trait Sealed<C>
where
    C: Decider,
{
}

impl<C, E> Sealed<C> for Events<E>
where
    C: Decider,
    E: Into<C::Event>,
{
}

impl<C> Sealed<C> for Vec<C::Event> where C: Decider {}

impl<C, E, const N: usize> Sealed<C> for [E; N]
where
    C: Decider,
    E: Into<C::Event>,
{
}
