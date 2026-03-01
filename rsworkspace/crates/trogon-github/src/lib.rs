//! # trogon-github
//!
//! GitHub webhook receiver that publishes events to NATS JetStream.
//!
//! ## How it works
//!
//! 1. GitHub sends `POST /webhook` with `X-Hub-Signature-256`, `X-GitHub-Event`,
//!    and `X-GitHub-Delivery` headers plus a JSON payload.
//! 2. The server validates the HMAC-SHA256 signature against `GITHUB_WEBHOOK_SECRET`.
//! 3. Events are published to NATS JetStream on `github.{event}` subjects
//!    (e.g. `github.push`, `github.pull_request`).
//! 4. The JetStream stream (`GITHUB` by default, capturing `github.>`) is created
//!    automatically on startup if it doesn't exist.
//!
//! ## NATS message format
//!
//! - **Subject**: `{GITHUB_SUBJECT_PREFIX}.{X-GitHub-Event}` (e.g. `github.push`)
//! - **Headers**: `X-GitHub-Event`, `X-GitHub-Delivery`
//! - **Payload**: raw JSON body from GitHub
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `GITHUB_WEBHOOK_SECRET` | — | HMAC-SHA256 secret (omit to skip validation) |
//! | `GITHUB_WEBHOOK_PORT` | `8080` | HTTP listening port |
//! | `GITHUB_SUBJECT_PREFIX` | `github` | NATS subject prefix |
//! | `GITHUB_STREAM_NAME` | `GITHUB` | JetStream stream name |
//! | `GITHUB_STREAM_MAX_AGE_SECS` | `604800` | Max age of messages in JetStream (seconds, default 7 days) |
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |

pub mod config;
pub mod server;
pub mod signature;

pub use config::GithubConfig;
pub use server::serve;
