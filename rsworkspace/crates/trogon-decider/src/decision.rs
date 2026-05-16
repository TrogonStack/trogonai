use std::{error::Error, fmt};

use super::{Act, ActBuilder, Decider, Events};

/// Outcome of [`Decider::decide`].
///
/// A decision either carries a concrete batch of [`Events`] or a deferred plan
/// ([`Act`]) whose steps run lazily so each step observes the state produced by the
/// previous step's events.
#[derive(Debug)]
#[non_exhaustive]
pub enum Decision<C>
where
    C: Decider,
{
    /// One or more events to be applied in order.
    Events(Events<C::Event>),
    /// A multi-step plan, evaluated against the evolving state.
    Act(Act<C>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(hidden)]
pub enum DecisionFailure<DecideError, EvolveError> {
    Decide(DecideError),
    Evolve(EvolveError),
}

impl<DecideError, EvolveError> fmt::Display for DecisionFailure<DecideError, EvolveError>
where
    DecideError: fmt::Display,
    EvolveError: fmt::Display,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decide(error) => write!(formatter, "decide failed: {error}"),
            Self::Evolve(error) => write!(formatter, "evolve failed: {error}"),
        }
    }
}

impl<DecideError, EvolveError> Error for DecisionFailure<DecideError, EvolveError>
where
    DecideError: Error + 'static,
    EvolveError: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Decide(error) => Some(error),
            Self::Evolve(error) => Some(error),
        }
    }
}

#[doc(hidden)]
pub type DecisionResult<C> = Result<
    (<C as Decider>::State, Events<<C as Decider>::Event>),
    DecisionFailure<<C as Decider>::DecideError, <C as Decider>::EvolveError>,
>;

impl<C> Decision<C>
where
    C: Decider,
{
    /// Wraps a single event as a decision.
    pub fn event(event: impl Into<C::Event>) -> Self {
        Self::Events(Events::one(event.into()))
    }

    /// Wraps a non-empty batch of events as a decision.
    pub fn events(events: Events<C::Event>) -> Self {
        Self::Events(events)
    }

    /// Starts a multi-step decision builder.
    ///
    /// Add steps with [`ActBuilder::execute`], then terminate the chain by converting
    /// it into a `Result<Decision<C>, C::DecideError>` with `.into()`. Each step
    /// observes the state that would result from applying every previous step's events.
    ///
    /// # Example
    ///
    /// ```
    /// # use trogon_decider::{Decider, Decision};
    /// # struct PlaceAndDiscount { order_id: String }
    /// # #[derive(Debug, PartialEq, Eq)]
    /// # enum OrderEvent {
    /// #     Placed { order_id: String },
    /// #     Discounted { order_id: String },
    /// # }
    /// # #[derive(Debug, PartialEq, Eq)]
    /// # enum OrderState { New, Placed }
    /// # #[derive(Debug, PartialEq, Eq)]
    /// # enum PlaceAndDiscountError { NotPlaced }
    /// # impl std::fmt::Display for PlaceAndDiscountError {
    /// #     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    /// #         match self {
    /// #             Self::NotPlaced => f.write_str("order is not placed"),
    /// #         }
    /// #     }
    /// # }
    /// # impl std::error::Error for PlaceAndDiscountError {}
    /// # #[derive(Debug, PartialEq, Eq)]
    /// # struct PlaceAndDiscountEvolveError;
    /// # impl std::fmt::Display for PlaceAndDiscountEvolveError {
    /// #     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    /// #         f.write_str("invalid order event")
    /// #     }
    /// # }
    /// # impl std::error::Error for PlaceAndDiscountEvolveError {}
    /// # impl Decider for PlaceAndDiscount {
    /// #     type StreamId = str;
    /// #     type State = OrderState;
    /// #     type Event = OrderEvent;
    /// #     type DecideError = PlaceAndDiscountError;
    /// #     type EvolveError = PlaceAndDiscountEvolveError;
    /// #     fn stream_id(&self) -> &str { &self.order_id }
    /// #     fn initial_state() -> OrderState { OrderState::New }
    /// #     fn evolve(_: OrderState, event: &OrderEvent) -> Result<OrderState, PlaceAndDiscountEvolveError> {
    /// #         match event { OrderEvent::Placed { .. } | OrderEvent::Discounted { .. } => Ok(OrderState::Placed) }
    /// #     }
    /// #     fn decide(_: &OrderState, cmd: &Self) -> Result<Decision<Self>, PlaceAndDiscountError> {
    /// #         Ok(Decision::event(OrderEvent::Placed { order_id: cmd.order_id.clone() }))
    /// #     }
    /// # }
    /// let decision: Result<Decision<PlaceAndDiscount>, _> =
    ///     Decision::<PlaceAndDiscount>::act()
    ///         .execute(|_state, cmd| Decision::event(OrderEvent::Placed {
    ///             order_id: cmd.order_id.clone(),
    ///         }))
    ///         .execute(|state, cmd| match state {
    ///             OrderState::Placed => Ok(Decision::event(OrderEvent::Discounted {
    ///                 order_id: cmd.order_id.clone(),
    ///             })),
    ///             OrderState::New => Err(PlaceAndDiscountError::NotPlaced),
    ///         })
    ///         .into();
    /// assert!(decision.is_ok());
    /// ```
    pub const fn act() -> ActBuilder<C> {
        ActBuilder::new()
    }

    #[doc(hidden)]
    pub fn handle(self, mut state: C::State, command: &C) -> DecisionResult<C> {
        match self {
            Self::Events(events) => {
                for event in events.iter() {
                    state = C::evolve(state, event).map_err(DecisionFailure::Evolve)?;
                }

                Ok((state, events))
            }
            Self::Act(act) => act.run(state, command),
        }
    }
}

impl<C> From<Decision<C>> for Result<Decision<C>, C::DecideError>
where
    C: Decider,
{
    fn from(decision: Decision<C>) -> Self {
        Ok(decision)
    }
}
