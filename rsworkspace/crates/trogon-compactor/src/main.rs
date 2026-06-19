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

use trogon_compactor::AuthStyle;
use trogon_compactor::detector::CompactionSettings;
use trogon_compactor::service::{self, ProviderConfig, ServiceState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "trogon_compactor=info".into()),
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

    // Each provider talks to its API the same way the runners do, so the compactor
    // works in the same deployment (proxy or proxy-less) without extra setup:
    // - Anthropic: direct (`x-api-key`) when `ANTHROPIC_BASE_URL` is set, else via
    //   the secret-proxy (`Authorization: Bearer`). Mirrors acp-runner.
    // - xAI / OpenRouter (OpenAI-compatible): default to their direct public URLs
    //   with `Bearer`, matching xai-runner / openrouter-runner. Point `*_BASE_URL`
    //   at `{proxy}/<provider>/v1` to route through the secret-proxy instead.
    // Tokens fall back to the runners' own env vars so the same credentials work.

    let anthropic = std::env::var("ANTHROPIC_TOKEN").ok().map(|token| {
        let (api_url, auth_style) = match std::env::var("ANTHROPIC_BASE_URL") {
            // Direct: real API keys (`sk-ant-api…`) authenticate with `x-api-key`;
            // OAuth tokens (`sk-ant-oat…`, the trogon default) use `Authorization:
            // Bearer`, matching how the runners call Anthropic in agent_loop.
            Ok(base) => {
                let auth = if token.starts_with("sk-ant-api") {
                    AuthStyle::XApiKey
                } else {
                    AuthStyle::Bearer
                };
                (format!("{}/messages", base.trim_end_matches('/')), auth)
            }
            // Via secret-proxy: always Bearer with the `tok_…` reference.
            Err(_) => (format!("{proxy}/anthropic/v1/messages"), AuthStyle::Bearer),
        };
        ProviderConfig {
            api_url,
            token,
            auth_style,
            default_model: std::env::var("COMPACTOR_MODEL").unwrap_or_else(|_| "claude-haiku-4-5-20251001".into()),
        }
    });
    let xai = std::env::var("COMPACTOR_XAI_TOKEN")
        .or_else(|_| std::env::var("XAI_API_KEY"))
        .ok()
        .map(|token| {
            let base = std::env::var("COMPACTOR_XAI_BASE_URL")
                .or_else(|_| std::env::var("XAI_BASE_URL"))
                .unwrap_or_else(|_| "https://api.x.ai/v1".into());
            ProviderConfig {
                api_url: format!("{}/chat/completions", base.trim_end_matches('/')),
                token,
                auth_style: AuthStyle::Bearer,
                default_model: std::env::var("COMPACTOR_XAI_MODEL").unwrap_or_else(|_| "grok-3".into()),
            }
        });
    let openrouter = std::env::var("COMPACTOR_OPENROUTER_TOKEN")
        .or_else(|_| std::env::var("OPENROUTER_API_KEY"))
        .ok()
        .map(|token| {
            let base = std::env::var("COMPACTOR_OPENROUTER_BASE_URL")
                .or_else(|_| std::env::var("OPENROUTER_BASE_URL"))
                .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());
            ProviderConfig {
                api_url: format!("{}/chat/completions", base.trim_end_matches('/')),
                token,
                auth_style: AuthStyle::Bearer,
                default_model: std::env::var("COMPACTOR_OPENROUTER_MODEL")
                    .unwrap_or_else(|_| "anthropic/claude-3.5-haiku".into()),
            }
        });

    // OpenAI (codex runner): OpenAI-compatible Chat Completions. Token falls back
    // to the codex runner's own OPENAI_API_KEY so the same credentials work.
    let openai = std::env::var("COMPACTOR_OPENAI_TOKEN")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .ok()
        .map(|token| {
            let base = std::env::var("COMPACTOR_OPENAI_BASE_URL")
                .or_else(|_| std::env::var("OPENAI_BASE_URL"))
                .unwrap_or_else(|_| "https://api.openai.com/v1".into());
            ProviderConfig {
                api_url: format!("{}/chat/completions", base.trim_end_matches('/')),
                token,
                auth_style: AuthStyle::Bearer,
                default_model: std::env::var("COMPACTOR_OPENAI_MODEL")
                    .unwrap_or_else(|_| "gpt-4o-mini".into()),
            }
        });

    if anthropic.is_none() && xai.is_none() && openrouter.is_none() && openai.is_none() {
        eprintln!(
            "error: no provider token configured — set at least one of ANTHROPIC_TOKEN, \
             COMPACTOR_XAI_TOKEN, COMPACTOR_OPENROUTER_TOKEN, COMPACTOR_OPENAI_TOKEN"
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
        openai,
    };

    service::run(nats, state).await?;

    Ok(())
}
