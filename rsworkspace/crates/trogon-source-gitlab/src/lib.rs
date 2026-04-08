//! # trogon-source-gitlab
//!
//! GitLab webhook receiver that publishes events to NATS JetStream.
//!
//! ## How it works
//!
//! 1. GitLab sends `POST /webhook` with `X-Gitlab-Token`, `X-GitLab-Event`,
//!    `X-Gitlab-Webhook-UUID`, `Idempotency-Key`, and `X-Gitlab-Instance`
//!    headers plus a JSON payload.
//! 2. The server validates the `X-Gitlab-Token` header against `GITLAB_WEBHOOK_SECRET`.
//! 3. Events are published to NATS JetStream on `gitlab.{event}` subjects
//!    (e.g. `gitlab.push`, `gitlab.merge_request_hook`).
//! 4. The JetStream stream (`GITLAB` by default, capturing `gitlab.>`) is created
//!    automatically on startup if it doesn't exist.
//! 5. Unroutable payloads (missing/invalid event header) are published to
//!    `{prefix}.unroutable` with an `X-GitLab-Reject-Reason` header.
//!
//! ## NATS message format
//!
//! - **Subject**: `{GITLAB_SUBJECT_PREFIX}.{X-GitLab-Event}` (e.g. `gitlab.push`)
//! - **Headers**: `X-GitLab-Event`, `X-GitLab-Webhook-UUID`, `X-GitLab-Instance`
//! - **Dedup**: `Nats-Msg-Id` set from `Idempotency-Key` (stable across retries)
//! - **Payload**: raw JSON body from GitLab
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `GITLAB_WEBHOOK_SECRET` | **required** | Shared secret configured in GitLab |
//! | `GITLAB_WEBHOOK_PORT` | `8080` | HTTP listening port |
//! | `GITLAB_SUBJECT_PREFIX` | `gitlab` | NATS subject prefix (must be valid NATS token) |
//! | `GITLAB_STREAM_NAME` | `GITLAB` | JetStream stream name (must be valid NATS token) |
//! | `GITLAB_STREAM_MAX_AGE_SECS` | `604800` | Max age of messages in JetStream (seconds, default 7 days) |
//! | `GITLAB_NATS_ACK_TIMEOUT_MS` | `10000` | NATS publish ack timeout in milliseconds |
//! | `GITLAB_MAX_BODY_SIZE` | `26214400` | Maximum webhook body size in bytes (default 25 MB) |
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |

pub mod config;
pub mod constants;
pub mod server;
pub mod signature;
pub use config::GitlabConfig;
pub use server::{provision, router};
pub use signature::SignatureError;
