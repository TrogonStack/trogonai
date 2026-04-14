//! NATS request-reply service for context compaction.
//!
//! Listens on `trogon.compactor.compact` and compacts incoming conversation
//! histories on demand.  Any trogon service (e.g. `trogon-acp-runner`) can
//! request compaction by sending a NATS request to this subject.
//!
//! ## Request / response
//!
//! **Request** (JSON):
//! ```json
//! {
//!   "messages": [{ "role": "user", "content": [...] }, ...]
//! }
//! ```
//!
//! **Response on success** (JSON):
//! ```json
//! {
//!   "messages": [...],
//!   "compacted": true,
//!   "tokens_before": 185000,
//!   "tokens_after": 22000
//! }
//! ```
//!
//! **Response on error** (JSON):
//! ```json
//! { "error": "..." }
//! ```

use async_nats::Client;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::error::CompactorError;
use crate::tokens::estimate_total_tokens;
use crate::{Compactor, Message};

/// NATS subject on which the compactor listens for requests.
pub const COMPACT_SUBJECT: &str = "trogon.compactor.compact";

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CompactRequest {
    pub messages: Vec<Message>,
}

#[derive(Serialize)]
pub struct CompactResponse {
    pub messages: Vec<Message>,
    /// `true` if old messages were actually replaced by a summary.
    pub compacted: bool,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ── Service ───────────────────────────────────────────────────────────────────

/// Runs the compactor as a NATS service until the connection drops.
///
/// Each incoming request is handled synchronously (one at a time).  For
/// higher throughput, spawn multiple instances behind a NATS queue group.
pub async fn run(nats: Client, compactor: Compactor) -> Result<(), async_nats::Error> {
    let mut sub = nats.subscribe(COMPACT_SUBJECT).await?;
    info!(subject = COMPACT_SUBJECT, "compactor service listening");

    while let Some(msg) = sub.next().await {
        let Some(reply) = msg.reply else {
            warn!("received fire-and-forget message on compact subject, ignoring");
            continue;
        };

        let response_bytes = match handle(&compactor, &msg.payload).await {
            Ok(resp) => serde_json::to_vec(&resp).unwrap_or_default(),
            Err(e) => {
                error!(error = %e, "compaction failed");
                serde_json::to_vec(&ErrorResponse { error: e.to_string() })
                    .unwrap_or_default()
            }
        };

        nats.publish(reply, response_bytes.into()).await.ok();
    }

    Ok(())
}

async fn handle(
    compactor: &Compactor,
    payload: &[u8],
) -> Result<CompactResponse, CompactorError> {
    let req: CompactRequest = serde_json::from_slice(payload)
        .map_err(|e| CompactorError::InvalidRequest(e.to_string()))?;

    let tokens_before = estimate_total_tokens(&req.messages);
    let messages = compactor.compact_if_needed(req.messages).await?;
    let tokens_after = estimate_total_tokens(&messages);

    Ok(CompactResponse {
        compacted: tokens_after < tokens_before,
        messages,
        tokens_before,
        tokens_after,
    })
}
