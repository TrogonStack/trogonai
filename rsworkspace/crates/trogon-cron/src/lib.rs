//! # trogon-cron
//!
//! Distributed CRON scheduler for TrogonStack applications.
//!
//! ## Features
//!
//! - Job definitions stored in NATS KV (`cron_configs` bucket) — hot-reloaded in real time.
//! - Two schedule modes: fixed interval (`interval_sec`) or 6-field cron expression.
//! - Two action modes: publish a NATS message or spawn a process.
//! - Leader election via NATS KV TTL — only one scheduler instance fires ticks.
//! - Graceful shutdown: releases the leader lock immediately on Ctrl-C / SIGTERM.
//! - Missed ticks on leader transition are skipped (at-most-once delivery).
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use trogon_cron::Scheduler;
//!
//! #[tokio::main]
//! async fn main() {
//!     let nats = async_nats::connect("nats://localhost:4222").await.unwrap();
//!     Scheduler::new(nats).run().await.unwrap();
//! }
//! ```
//!
//! ## Job config example (JSON stored in NATS KV under key `jobs.backup`)
//!
//! ```json
//! {
//!   "id": "backup",
//!   "schedule": { "type": "interval", "interval_sec": 3600 },
//!   "action":   { "type": "publish",  "subject": "cron.backup" },
//!   "enabled":  true,
//!   "payload":  { "db": "main" }
//! }
//! ```
//!
//! ```json
//! {
//!   "id": "report",
//!   "schedule": { "type": "cron", "expr": "0 0 8 * * *" },
//!   "action":   { "type": "spawn", "bin": "/usr/bin/report", "args": ["--format", "pdf"], "concurrent": false, "timeout_sec": 120 },
//!   "enabled":  true
//! }
//! ```

pub mod config;
pub mod error;
pub mod executor;
pub mod kv;
pub mod leader;
pub mod scheduler;

pub use config::{Action, JobConfig, Schedule, TickPayload};
pub use error::CronError;
pub use scheduler::Scheduler;
