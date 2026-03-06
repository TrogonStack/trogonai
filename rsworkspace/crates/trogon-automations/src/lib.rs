//! `trogon-automations` — dynamic automation configuration over NATS KV.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────┐      PATCH /enable        ┌──────────────────┐
//! │   Frontend   │ ─── POST /automations ───▶ │  AutomationStore │
//! └──────────────┘      DELETE /:id           │  (NATS KV)       │
//!                                             └────────┬─────────┘
//!                                                      │ watch()
//!                                                      ▼
//! ┌──────────────┐   github.pull_request   ┌──────────────────────┐
//! │  trogon-     │ ───────────────────────▶│  Runner (in-memory   │
//! │  github      │   linear.Issue.create   │  cache + KV watch)   │
//! └──────────────┘ ───────────────────────▶└────────┬─────────────┘
//!                                                   │ matching automations
//!                                                   ▼
//!                                        AgentLoop (per automation, parallel)
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! use trogon_automations::{AutomationStore, api};
//!
//! // Open the store (creates the KV bucket if needed).
//! let store = AutomationStore::open(&js).await?;
//!
//! // Start the management API.
//! let state = api::AppState { store: store.clone() };
//! let listener = tokio::net::TcpListener::bind("0.0.0.0:8090").await?;
//! tokio::spawn(async move { axum::serve(listener, api::router(state)).await });
//!
//! // In the runner: find automations matching an incoming event.
//! let matches = store.matching("github.pull_request", &payload).await?;
//! for auto in matches {
//!     // run agent with auto.prompt, auto.tools, auto.memory_path …
//! }
//! ```

pub mod api;
pub mod automation;
pub mod store;
pub mod trigger;

pub use automation::{Automation, McpServer};
pub use store::AutomationStore;
