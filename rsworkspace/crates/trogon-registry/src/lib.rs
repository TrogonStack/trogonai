//! # trogon-registry
//!
//! Live, self-updating index of every registered agent and its capabilities.
//!
//! The registry is the mechanism that makes the multi-agent system *dynamic*:
//! instead of a hardcoded routing table, every agent type self-registers at
//! startup. The Router queries this registry at runtime, hands the list to
//! the LLM, and lets the LLM decide who handles the incoming event.
//!
//! ## How it works
//!
//! 1. On startup, each agent calls [`Registry::register`] with its
//!    [`AgentCapability`].
//! 2. A background task calls [`Registry::refresh`] every
//!    [`provision::HEARTBEAT_INTERVAL`] (15 s) to keep the entry alive.
//! 3. NATS KV automatically removes entries that have not been refreshed within
//!    [`provision::ENTRY_TTL`] (30 s), so crashed agents disappear automatically.
//! 4. The Router calls [`Registry::list_all`] or [`Registry::discover`] on each
//!    incoming event and feeds the result to the LLM routing prompt.
//!
//! ## Typical agent startup
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use trogon_registry::{AgentCapability, Registry, provision};
//!
//! async fn startup(js: async_nats::jetstream::Context) {
//!     let store = provision(&js).await.expect("registry provisioning failed");
//!     let registry = Registry::new(store.clone());
//!
//!     let cap = AgentCapability::new(
//!         "PrActor",
//!         ["code_review", "security_analysis"],
//!         "actors.pr.>",
//!     );
//!
//!     registry.register(&cap).await.expect("initial registration failed");
//!
//!     // Keep the entry alive in a background task.
//!     tokio::spawn({
//!         let cap = cap.clone();
//!         async move {
//!             let mut interval = tokio::time::interval(Duration::from_secs(15));
//!             loop {
//!                 interval.tick().await;
//!                 registry.refresh(&cap).await.ok();
//!             }
//!         }
//!     });
//! }
//! ```

pub mod capability;
pub mod error;
pub mod provision;
pub mod registry;
pub mod store;

pub use capability::AgentCapability;
pub use error::RegistryError;
pub use provision::{BUCKET_NAME, ENTRY_TTL, HEARTBEAT_INTERVAL, provision};
pub use registry::Registry;
pub use store::RegistryStore;

#[cfg(any(test, feature = "test-support"))]
pub use store::mock::MockRegistryStore;
