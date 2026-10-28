//! Slack source for `trogon-gateway`.
//!
//! Receives Slack Events API deliveries, interactive payloads and slash commands
//! over either an HTTP webhook or Socket Mode and publishes each one to NATS
//! JetStream exactly as Slack sent it.
//!
//! ## How it works
//!
//! 1. An integration is configured with exactly one transport. The webhook
//!    transport mounts `/sources/slack/{integration}/webhook`. Socket Mode opens
//!    Slack's WebSocket with an app-level token.
//! 2. HTTP requests are checked against the `X-Slack-Request-Timestamp` drift
//!    window and the HMAC-SHA256 `X-Slack-Signature`. Socket Mode relies on
//!    Slack's pre-authenticated connection.
//! 3. `url_verification` challenges and `ssl_check=1` form posts are answered
//!    inline and never published. `ssl_check` is answered before signature
//!    verification, as Slack sends it unsigned.
//! 4. Everything else is published to JetStream. The delivery is acknowledged
//!    to Slack (HTTP 200, or an envelope ack over Socket Mode) only after the
//!    publish was stored, within the time Slack allows for an acknowledgement.
//! 5. A payload that cannot be routed is stored on the unroutable subject with
//!    the reason in a header and the original bytes as the body. If that store
//!    fails too, the delivery is not acknowledged so Slack redelivers it.
//! 6. The JetStream stream is created on startup if it does not exist. Its
//!    subjects, `max_age` and dedup window are reconciled on every start, and
//!    settings an operator changed out of band, such as replicas, are left alone.
//!
//! ## NATS message format
//!
//! | Payload | Subject |
//! |---|---|
//! | `event_callback` | `{prefix}.event.{event.type}` |
//! | Interaction | `{prefix}.interaction.{type}` |
//! | Slash command | `{prefix}.command.{name}` without the leading `/` |
//! | System notice | `{prefix}.system.{type}`, today only `app_rate_limited` |
//! | Unroutable | `{prefix}.unroutable` |
//!
//! Event types, interaction types and command names come from the sender, so
//! one that is not a single valid subject token is treated as unroutable.
//!
//! - **Payload**: the bytes Slack sent. Socket Mode publishes the envelope's
//!   `payload` slice without re-serializing it.
//! - **Content-Type**: `application/x-www-form-urlencoded` for slash commands
//!   received over HTTP, `application/json` for every other routed message, and
//!   `application/octet-stream` for unroutable messages, whose bytes are kept as
//!   received.
//! - **Headers**: `Nats-Msg-Id`, `X-Slack-Payload-Kind`, `X-Slack-Event-Type`,
//!   `X-Slack-Team-Id`, `X-Slack-Transport`, plus `X-Slack-Event-Id` for events,
//!   `X-Slack-Retry-Num` and `X-Slack-Retry-Reason` when Slack marks the
//!   delivery as a redelivery, `X-Slack-Is-Enterprise-Install` always,
//!   `X-Slack-Enterprise-Id` and `X-Slack-Api-App-Id` when the payload names
//!   them, `X-Slack-Event-Subtype` and `X-Slack-Channel-Type` for events that
//!   carry them, `X-Slack-Envelope-Id` over Socket Mode,
//!   `X-Slack-Request-Timestamp` over HTTP, and `X-Slack-Reject-Reason` for
//!   unroutable messages. System notices carry `X-Slack-Payload-Kind: system`
//!   and the notice type in `X-Slack-Event-Type`, and their `Nats-Msg-Id` is
//!   `system-` followed by the SHA-256 of the body because they have no event id.
//! - **Channel type**: `X-Slack-Channel-Type` copies Slack's `channel_type`
//!   as sent, and its values depend on the event. Message events send names
//!   such as `channel` or `im`, while membership events send single letter
//!   codes such as `C`, so filter on it together with `X-Slack-Event-Type`.
//!
//! ## Configuration
//!
//! Configured in TOML under `[sources.slack.integrations.{id}]`. See the crate
//! README for the shared fields. Slack specific settings:
//!
//! | Key | Default | Description |
//! |---|---|---|
//! | `webhook.signing_secret` | required | Slack app signing secret |
//! | `webhook.timestamp_max_drift_secs` | `300` | Allowed clock drift for request timestamps |
//! | `webhook.max_body_bytes` | `4194304` | Largest accepted request body |
//! | `socket_mode.app_token` | required | App-level token, starts with `xapp-` |
//! | `socket_mode.api_base_url` | `https://slack.com/api/` | Slack Web API base for `apps.connections.open`, for example `https://slack-gov.com/api/` for GovSlack |
//! | `nats_ack_timeout_secs` | derived from Slack's 3 second deadline | Capped at that budget |

pub mod ack_budget;
pub mod config;
pub mod constants;
pub mod dedup_window;
pub mod delivery_metadata;
pub mod header_text;
pub mod interaction_key;
pub mod payload_encoding;
pub mod payload_kind;
pub mod payload_metadata;
pub mod retry_attempt;
pub mod retry_reason;
pub mod server;
pub mod signature;
pub mod socket_mode;
pub mod ssl_check;
pub mod subject;
pub mod system_notice;
pub mod unroutable;

#[cfg(test)]
mod bolt_conformance_tests;

pub use config::SlackConfig;
pub use server::{provision, router};
