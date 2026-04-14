//! # trogon-actor
//!
//! The Entity Actor primitive for the dynamic multi-agent system.
//!
//! An Entity Actor is a stateful agent bound to the full lifecycle of a single
//! domain entity — a PR, an incident, a deployment, a support ticket. Its
//! `State` persists across **all** events that ever touch that entity, giving
//! it continuous memory:
//!
//! > *Monday: PR opens → actor reviews, saves issues found.*
//! > *Wednesday: developer pushes fix → actor loads Monday's state, reviews incrementally.*
//! > *Friday: CI fails → actor correlates with the issue it flagged Wednesday.*
//!
//! ## Key components
//!
//! | Component | Role |
//! |---|---|
//! | [`EntityActor`] | Trait your actor implements: `handle(state, ctx)` |
//! | [`ActorContext`] | Passed to `handle`: transcript helpers + `spawn_agent` |
//! | [`ActorRuntime`] | Orchestrates load → handle → save with OCC retry |
//! | [`StateStore`] | KV abstraction for actor state (real: `kv::Store`, test: `MockStateStore`) |
//! | [`provision_state`] | Creates the `ACTOR_STATE` KV bucket |
//!
//! ## Minimal example
//!
//! ```rust,no_run
//! use serde::{Deserialize, Serialize};
//! use trogon_actor::{ActorContext, ActorRuntime, EntityActor, provision_state};
//! use trogon_registry::{AgentCapability, Registry, provision as provision_registry};
//! use trogon_transcript::publisher::NatsTranscriptPublisher;
//!
//! #[derive(Default, Serialize, Deserialize)]
//! struct PrState { reviews: u32 }
//!
//! struct PrActor;
//!
//! impl EntityActor for PrActor {
//!     type State = PrState;
//!     type Error = std::convert::Infallible;
//!     fn actor_type() -> &'static str { "pr" }
//!     async fn handle(&mut self, state: &mut PrState, ctx: &ActorContext)
//!         -> Result<(), Self::Error>
//!     {
//!         state.reviews += 1;
//!         ctx.append_user_message("reviewing", None).await.ok();
//!         Ok(())
//!     }
//! }
//!
//! async fn run(nats: async_nats::Client, js: async_nats::jetstream::Context) {
//!     let state_store  = provision_state(&js).await.unwrap();
//!     let reg_store    = provision_registry(&js).await.unwrap();
//!     let publisher    = NatsTranscriptPublisher::new(js);
//!     let registry     = Registry::new(reg_store);
//!     let mut runtime  = ActorRuntime::new(state_store, publisher, nats, registry);
//!     let mut actor    = PrActor;
//!     runtime.handle_event(&mut actor, "owner/repo/456").await.unwrap();
//! }
//! ```

pub mod actor;
pub mod context;
pub mod error;
pub mod runtime;
pub mod state;

pub use actor::EntityActor;
pub use context::ActorContext;
pub use error::{ActorError, SaveError};
pub use runtime::ActorRuntime;
pub use state::{
    MAX_OCC_RETRIES, StateStore, provision_state, state_kv_key,
};

#[cfg(any(test, feature = "test-helpers"))]
pub use context::test_helpers::ContextBuilder;
#[cfg(any(test, feature = "test-helpers"))]
pub use state::mock::MockStateStore;
