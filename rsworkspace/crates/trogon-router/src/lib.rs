//! # trogon-router
//!
//! The Router Agent for the TrogonStack Dynamic Multi-Agent System.
//!
//! Subscribes to `trogon.events.>` on NATS, calls an LLM to decide which
//! Entity Actor should handle each incoming event, records the routing
//! decision in the transcript, and forwards the payload to the actor's
//! NATS subject.
//!
//! ## Architecture
//!
//! ```text
//! NATS (trogon.events.>)
//!        │
//!        ▼
//!   Router Agent
//!   ├── Registry lookup  (which agents are alive?)
//!   ├── LLM call         (which agent + entity key for this event?)
//!   ├── Transcript write (record routing decision)
//!   └── NATS publish     (forward to actors.{type}.{entity_key})
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use trogon_router::{Router, llm::{LlmConfig, OpenAiCompatClient}};
//! use trogon_registry::{Registry, provision as provision_registry};
//! use trogon_transcript::{NatsTranscriptPublisher};
//!
//! async fn example(
//!     js: async_nats::jetstream::Context,
//!     nats: async_nats::Client,
//! ) {
//!     let store = provision_registry(&js).await.unwrap();
//!     let registry = Registry::new(store);
//!
//!     let http = reqwest::Client::new();
//!     let llm_config = LlmConfig {
//!         api_url: "https://api.x.ai/v1".into(),
//!         api_key: std::env::var("XAI_API_KEY").unwrap(),
//!         model: "grok-3-mini".into(),
//!     };
//!     let llm = OpenAiCompatClient::new(http, llm_config);
//!     let publisher = NatsTranscriptPublisher::new(js);
//!
//!     let router = Router::new(llm, registry, publisher, nats);
//!     router.run("trogon.events.>").await.unwrap();
//! }
//! ```

pub mod decision;
pub mod error;
pub mod event;
pub mod llm;
pub mod metrics;
pub mod prompt;
pub mod router;
pub mod unroutable;

pub use decision::{RouteResult, RoutingDecision};
pub use error::RouterError;
pub use event::RouterEvent;
pub use llm::{LLM_REQUEST_TIMEOUT, LlmClient, LlmConfig, OpenAiCompatClient};
pub use router::Router;
