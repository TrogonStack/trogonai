use serde::{Deserialize, Serialize};
use std::future::Future;

use crate::context::ActorContext;

/// The trait every Entity Actor must implement.
///
/// An Entity Actor is a stateful agent tied to the full lifecycle of one domain
/// entity — a PR, an incident, a deployment, etc. Its `State` persists across
/// all events for that entity. When a new event arrives the runtime:
///
/// 1. Loads the entity's `State` from the NATS KV store (or calls
///    [`on_create`][EntityActor::on_create] if none exists yet).
/// 2. Opens a new transcript session.
/// 3. Calls [`handle`][EntityActor::handle] with mutable access to state and
///    an [`ActorContext`] for transcript recording and sub-agent spawning.
/// 4. Saves the updated state back to KV with optimistic concurrency. If a
///    conflict is detected (two events landed simultaneously) the runtime
///    retries the whole cycle from step 1.
///
/// # Example
///
/// ```rust,no_run
/// use serde::{Deserialize, Serialize};
/// use trogon_actor::{EntityActor, ActorContext};
///
/// #[derive(Default, Serialize, Deserialize)]
/// pub struct PrState {
///     pub reviews_count: u32,
///     pub last_issues: Vec<String>,
/// }
///
/// pub struct PrActor;
///
/// impl EntityActor for PrActor {
///     type State = PrState;
///     type Error = std::convert::Infallible;
///
///     fn actor_type() -> &'static str { "pr" }
///
///     async fn handle(
///         &mut self,
///         state: &mut PrState,
///         ctx: &ActorContext,
///     ) -> Result<(), Self::Error> {
///         state.reviews_count += 1;
///         ctx.append_user_message("Reviewing new commits", None).await.ok(); // transcript, non-fatal
///         Ok(())
///     }
/// }
/// ```
#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ActorContext;

    #[derive(Default, serde::Serialize, serde::Deserialize)]
    struct Empty;

    struct Minimal;

    impl EntityActor for Minimal {
        type State = Empty;
        type Error = std::convert::Infallible;

        fn actor_type() -> &'static str {
            "minimal"
        }

        async fn handle(
            &mut self,
            _state: &mut Empty,
            _ctx: &ActorContext,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn on_create_default_impl_is_noop() {
        let mut state = Empty;
        Minimal::on_create(&mut state).await.unwrap();
    }

    #[tokio::test]
    async fn on_destroy_default_impl_is_noop() {
        let mut state = Empty;
        Minimal::on_destroy(&mut state).await.unwrap();
    }

    /// Run `Minimal` through the real runtime so that `handle()` is executed
    /// and the default `on_create` path is exercised through the runtime cycle.
    #[tokio::test]
    async fn minimal_actor_handle_is_callable() {
        use crate::{
            runtime::ActorRuntime,
            state::mock::MockStateStore,
        };
        use trogon_registry::{MockRegistryStore, Registry};
        use trogon_transcript::publisher::mock::MockTranscriptPublisher;

        let runtime = ActorRuntime::new(
            MockStateStore::new(),
            MockTranscriptPublisher::new(),
            trogon_nats::MockNatsClient::new(),
            Registry::new(MockRegistryStore::new()),
            trogon_nats::jetstream::MockJetStreamPublisher::new(),
        );
        runtime.handle_event(&mut Minimal, "e-1", 0).await.unwrap();
    }
}

pub trait EntityActor: Send + 'static {
    /// The persistent state type. The runtime loads this from KV before each
    /// event and saves it back afterwards. Must implement `Default` (returned
    /// on first-ever event for this entity).
    type State: Serialize + for<'de> Deserialize<'de> + Default + Send;

    /// The error type returned by this actor's handler methods.
    type Error: std::error::Error + Send + Sync + 'static;

    /// A short, stable identifier for this actor type used as the KV key
    /// prefix and as the `actor_type` in transcript subjects.
    /// Example: `"pr"`, `"incident"`, `"deployment"`.
    fn actor_type() -> &'static str;

    /// Handle one incoming event.
    ///
    /// `state` is pre-loaded from KV (or freshly defaulted). Any mutations are
    /// automatically persisted after this method returns.
    ///
    /// `ctx` provides transcript recording and sub-agent spawning.
    fn handle(
        &mut self,
        state: &mut Self::State,
        ctx: &ActorContext,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Called exactly once when the very first event arrives for a new entity.
    /// Use this to set non-default initial state or emit a "created" transcript
    /// entry.
    fn on_create(
        _state: &mut Self::State,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        std::future::ready(Ok(()))
    }

    /// Called when the entity is destroyed (PR closed, incident resolved, …).
    /// The runtime does not call this automatically — it must be triggered
    /// explicitly by the actor itself via `on_destroy_self`.
    fn on_destroy(
        _state: &mut Self::State,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        std::future::ready(Ok(()))
    }
}
