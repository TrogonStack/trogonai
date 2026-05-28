//! `trogon-compactor` — context compaction service for long-running Claude sessions.
//!
//! ## Architecture
//!
//! ```text
//! trogon-acp-runner (or any trogon service)
//!         │  NATS request-reply
//!         ▼
//! trogon-compactor  [this binary]
//!     └─ Compactor::compact_if_needed()
//!          └─ summarizer::generate_summary()
//!                  │  HTTP POST
//!                  ▼
//!         trogon-secret-proxy → Anthropic API
//! ```
//!
//! ## Environment variables
//!
//! | Variable                          | Default                              | Description                              |
//! |-----------------------------------|--------------------------------------|------------------------------------------|
//! | `NATS_URL`                        | `nats://localhost:4222`              | NATS server URL                          |
//! | `PROXY_URL`                       | `http://localhost:8080`              | trogon-secret-proxy base URL             |
//! | `ANTHROPIC_TOKEN`                 | *(required)*                         | Proxy bearer token for Anthropic API     |
//! | `COMPACTOR_MODEL`                 | `claude-haiku-4-5-20251001`          | Model used for summarization             |
//! | `COMPACTOR_MAX_SUMMARY_TOKENS`    | `8192`                               | Max tokens in the generated summary      |
//! | `COMPACTOR_CONTEXT_WINDOW`        | `200000`                             | Target model context window size         |
//! | `COMPACTOR_RESERVE_TOKENS`        | `16384`                              | Tokens reserved for new output           |
//! | `COMPACTOR_KEEP_RECENT_TOKENS`    | `20000`                              | Minimum recent tokens kept verbatim      |

use tracing::info;

use trogon_compactor::detector::CompactionSettings;
use trogon_compactor::service::{self, ProviderConfig, ServiceState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "trogon_compactor=info".into()),
        )
        .init();

    // ── Config from environment ───────────────────────────────────────────────

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let proxy_url = std::env::var("PROXY_URL").unwrap_or_else(|_| "http://localhost:8080".into());
    let proxy = proxy_url.trim_end_matches('/');

    let max_summary_tokens: u32 = std::env::var("COMPACTOR_MAX_SUMMARY_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8_192);

    let context_window: usize = std::env::var("COMPACTOR_CONTEXT_WINDOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200_000);
    let reserve_tokens: usize = std::env::var("COMPACTOR_RESERVE_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16_384);
    let keep_recent_tokens: usize = std::env::var("COMPACTOR_KEEP_RECENT_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);

    // ── Per-provider config (only providers with a token are enabled) ─────────
    // ANTHROPIC_TOKEN kept for backward-compat. xAI / OpenRouter are optional.

    let anthropic = std::env::var("ANTHROPIC_TOKEN").ok().map(|token| ProviderConfig {
        api_url: format!("{proxy}/anthropic/v1/messages"),
        token,
        default_model: std::env::var("COMPACTOR_MODEL")
            .unwrap_or_else(|_| "claude-haiku-4-5-20251001".into()),
    });
    let xai = std::env::var("COMPACTOR_XAI_TOKEN").ok().map(|token| ProviderConfig {
        api_url: format!("{proxy}/xai/v1/chat/completions"),
        token,
        default_model: std::env::var("COMPACTOR_XAI_MODEL").unwrap_or_else(|_| "grok-2-1212".into()),
    });
    let openrouter = std::env::var("COMPACTOR_OPENROUTER_TOKEN").ok().map(|token| ProviderConfig {
        api_url: format!("{proxy}/openrouter/v1/chat/completions"),
        token,
        default_model: std::env::var("COMPACTOR_OPENROUTER_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-3.5-haiku".into()),
    });

    if anthropic.is_none() && xai.is_none() && openrouter.is_none() {
        eprintln!(
            "error: no provider token configured — set at least one of ANTHROPIC_TOKEN, \
             COMPACTOR_XAI_TOKEN, COMPACTOR_OPENROUTER_TOKEN"
        );
        std::process::exit(1);
    }

    // ── NATS connection ───────────────────────────────────────────────────────

    let nats = async_nats::connect(&nats_url).await?;
    info!(url = %nats_url, "connected to NATS");

    // ── Service state ─────────────────────────────────────────────────────────

    let state = ServiceState {
        client: reqwest::Client::new(),
        default_settings: CompactionSettings {
            context_window,
            reserve_tokens,
            keep_recent_tokens,
        },
        max_summary_tokens,
        anthropic,
        xai,
        openrouter,
    };

    service::run(nats, state).await?;

    Ok(())
}
