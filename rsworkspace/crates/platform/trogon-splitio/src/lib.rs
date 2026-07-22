//! Thin HTTP client for the [Split Evaluator] service.
//!
//! Split.io (Harness FME) has no official Rust SDK. The recommended path for
//! unsupported languages is to run the **Split Evaluator** as a sidecar
//! (`splitsoftware/split-evaluator` Docker image) and call it over HTTP.
//!
//! This crate wraps those HTTP calls with an ergonomic Rust API.
//!
//! # Architecture
//!
//! ```text
//! [your code]  →  SplitClient (HTTP)  →  split-evaluator :7548  →  split.io CDN
//! ```
//!
//! The Rust code never holds the server-side SDK key; that lives only inside
//! the evaluator container.  The client authenticates to the evaluator with a
//! separate `auth_token` (the `SPLIT_EVALUATOR_AUTH_TOKEN` env var you
//! configure on the evaluator and mirror here).
//!
//! # Quick start
//!
//! ```no_run
//! use trogon_splitio::{SplitClient, SplitConfig};
//! use trogon_std::env::SystemEnv;
//!
//! #[tokio::main]
//! async fn main() {
//!     let cfg = SplitConfig::from_env(&SystemEnv);
//!     let client = SplitClient::new(cfg);
//!
//!     let treatment = client
//!         .get_treatment("user-123", "new_checkout_flow", None)
//!         .await
//!         .unwrap();
//!
//!     if treatment == "on" {
//!         println!("New checkout enabled for user-123");
//!     }
//! }
//! ```
//!
//! [Split Evaluator]: https://help.split.io/hc/en-us/articles/360020037072-Split-Evaluator

pub mod client;
pub mod config;
pub mod error;
pub mod flags;
pub mod mock;

pub use client::{HttpClient, HttpResponse, SplitClient};
pub use config::SplitConfig;
pub use error::SplitError;
pub use flags::FeatureFlag;

/// The value returned when the SDK is not ready, a flag does not exist, or an
/// error occurred during evaluation.  Never roll out new behaviour to a user
/// who receives `"control"`.
pub const CONTROL: &str = "control";
