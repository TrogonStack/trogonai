use std::{fmt, marker::PhantomData};

use super::{Decider, Decision, DecisionFailure, DecisionResult};

/// Builder for a multi-step [`Decision::Act`].
///
/// Obtained via [`Decision::act`]. Call [`execute`](Self::execute) one or more times to
/// add steps, then convert the resulting chain into a `Result<Decision<C>, C::DecideError>`
/// with `.into()` to terminate.
///
/// The chain is monomorphized at compile time (each `execute` returns a new typestate
/// type), and the whole chain is boxed exactly once when it is converted into a
/// [`Decision::Act`].
pub struct ActBuilder<C>
where
    C: Decider,
{
    _decider: PhantomData<fn() -> C>,
}

impl<C> ActBuilder<C>
where
    C: Decider,
{
    pub(crate) const fn new() -> Self {
        Self { _decider: PhantomData }
    }

    /// Adds the first step to the chain.
    ///
    /// A step is a closure `FnOnce(&C::State, &C) -> D` where `D` is either a
    /// [`Decision<C>`] or a `Result<Decision<C>, C::DecideError>` — both convert via
    /// `Into`, so steps can return whichever is more natural for their logic.
    pub fn execute<F, D>(self, step: F) -> ActChain<C, First<F>>
    where
        F: FnOnce(&C::State, &C) -> D + 'static,
        D: Into<Result<Decision<C>, C::DecideError>>,
    {
        ActChain::new(First { step })
    }
}

impl<C> Default for ActBuilder<C>
where
    C: Decider,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<C> fmt::Debug for ActBuilder<C>
where
    C: Decider,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ActBuilder").finish_non_exhaustive()
    }
}

#[doc(hidden)]
pub struct ActChain<C, S>
where
    C: Decider,
    S: Steps<C>,
{
    steps: S,
    _decider: PhantomData<fn() -> C>,
}

impl<C, S> ActChain<C, S>
where
    C: Decider,
    S: Steps<C>,
{
    fn new(steps: S) -> Self {
        Self {
            steps,
            _decider: PhantomData,
        }
    }

    /// Adds another step to the chain.
    ///
    /// The step receives the state that would result from applying every previous
    /// step's events in order.
    pub fn execute<F, D>(self, step: F) -> ActChain<C, Then<S, F>>
    where
        F: FnOnce(&C::State, &C) -> D + 'static,
        D: Into<Result<Decision<C>, C::DecideError>>,
    {
        ActChain::new(Then { prev: self.steps, step })
    }
}

impl<C, S> From<ActChain<C, S>> for Result<Decision<C>, C::DecideError>
where
    C: Decider + 'static,
    S: Steps<C> + 'static,
{
    fn from(chain: ActChain<C, S>) -> Self {
        Ok(Decision::Act(Act::new(chain)))
    }
}

impl<C, S> fmt::Debug for ActChain<C, S>
where
    C: Decider,
    S: Steps<C>,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ActChain").finish_non_exhaustive()
    }
}

/// A type-erased multi-step decision plan.
///
/// Produced by terminating an [`ActBuilder`] chain. Each step inside the plan is
/// monomorphized; the plan itself is boxed once at this boundary so [`Decision`] can
/// hold it as a uniform type.
pub struct Act<C>
where
    C: Decider,
{
    inner: Box<dyn ActRun<C> + 'static>,
}

impl<C> Act<C>
where
    C: Decider,
{
    fn new<S>(chain: ActChain<C, S>) -> Self
    where
        C: 'static,
        S: Steps<C> + 'static,
    {
        Self { inner: Box::new(chain) }
    }

    pub(crate) fn run(self, state: C::State, command: &C) -> DecisionResult<C> {
        self.inner.run(state, command)
    }
}

impl<C> fmt::Debug for Act<C>
where
    C: Decider,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Act").finish_non_exhaustive()
    }
}

#[doc(hidden)]
pub trait ActRun<C>
where
    C: Decider,
{
    fn run(self: Box<Self>, state: C::State, command: &C) -> DecisionResult<C>;
}

impl<C, S> ActRun<C> for ActChain<C, S>
where
    C: Decider,
    S: Steps<C> + 'static,
{
    fn run(self: Box<Self>, state: C::State, command: &C) -> DecisionResult<C> {
        let Self { steps, .. } = *self;
        steps.run(state, command)
    }
}

#[doc(hidden)]
pub trait Steps<C>
where
    C: Decider,
{
    fn run(self, state: C::State, command: &C) -> DecisionResult<C>;
}

#[doc(hidden)]
pub struct First<F> {
    step: F,
}

impl<C, F, D> Steps<C> for First<F>
where
    C: Decider,
    F: FnOnce(&C::State, &C) -> D + 'static,
    D: Into<Result<Decision<C>, C::DecideError>>,
{
    fn run(self, state: C::State, command: &C) -> DecisionResult<C> {
        let result: Result<Decision<C>, C::DecideError> = (self.step)(&state, command).into();
        let decision = result.map_err(DecisionFailure::Decide)?;
        decision.handle(state, command)
    }
}

#[doc(hidden)]
pub struct Then<P, F> {
    prev: P,
    step: F,
}

impl<C, P, F, D> Steps<C> for Then<P, F>
where
    C: Decider,
    P: Steps<C>,
    F: FnOnce(&C::State, &C) -> D + 'static,
    D: Into<Result<Decision<C>, C::DecideError>>,
{
    fn run(self, state: C::State, command: &C) -> DecisionResult<C> {
        let (state, mut events) = self.prev.run(state, command)?;
        let result: Result<Decision<C>, C::DecideError> = (self.step)(&state, command).into();
        let decision = result.map_err(DecisionFailure::Decide)?;
        let (state, new_events) = decision.handle(state, command)?;
        events.extend(new_events);
        Ok((state, events))
    }
}
