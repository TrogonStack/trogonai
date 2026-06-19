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

use crate::detector::CompactionSettings;
use crate::error::CompactorError;
use crate::summarizer::{AuthStyle, LlmConfig, OpenAICompatLlmProvider};
use crate::tokens::estimate_total_tokens;
use crate::{AnthropicLlmProvider, Compactor, DynLlmProvider, Message};

/// NATS subject on which the compactor listens for requests.
pub const COMPACT_SUBJECT: &str = "trogon.compactor.compact";

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub struct CompactRequest {
    pub messages: Vec<Message>,
    /// `"anthropic"` | `"xai"` | `"openrouter"`. `None` → `"anthropic"`.
    /// Optional + default so old callers (only `{messages}`) keep working.
    #[serde(default)]
    pub provider: Option<String>,
    /// Session model. `None` → the provider's configured default model.
    #[serde(default)]
    pub model: Option<String>,
    /// Override model (same provider). Takes precedence over `model`.
    #[serde(default)]
    pub compactor_model: Option<String>,
    /// Target context window; when present, settings are derived from it.
    #[serde(default)]
    pub context_window: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CompactResponse {
    pub messages: Vec<Message>,
    /// `true` if old messages were actually replaced by a summary.
    pub compacted: bool,
    pub tokens_before: usize,
    pub tokens_after: usize,
    /// Number of trailing messages preserved verbatim (Gap 2: lets stateless
    /// runners reuse their own original tail). `messages.len()` when not compacted.
    pub kept_count: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ── Service state ───────────────────────────────────────────────────────────────

/// Per-provider connection details. The token is a `tok_...` secret-proxy bearer
/// token; the real API key never leaves the proxy worker.
#[derive(Clone)]
pub struct ProviderConfig {
    pub api_url: String,
    pub token: String,
    /// How the key is sent: `XApiKey` for direct Anthropic, `Bearer` for the
    /// secret-proxy and for direct xAI/OpenRouter (OpenAI-compatible) endpoints.
    pub auth_style: AuthStyle,
    pub default_model: String,
}

/// Holds the shared HTTP client + per-provider config. The handler builds the
/// right [`DynLlmProvider`] per request, reusing the client.
pub struct ServiceState {
    pub client: reqwest::Client,
    pub default_settings: CompactionSettings,
    pub max_summary_tokens: u32,
    pub anthropic: Option<ProviderConfig>,
    pub xai: Option<ProviderConfig>,
    pub openrouter: Option<ProviderConfig>,
    pub openai: Option<ProviderConfig>,
}

impl ServiceState {
    fn provider_config(&self, name: &str) -> Option<&ProviderConfig> {
        match name {
            "anthropic" => self.anthropic.as_ref(),
            "xai" => self.xai.as_ref(),
            "openrouter" => self.openrouter.as_ref(),
            "openai" => self.openai.as_ref(),
            _ => None,
        }
    }
}

/// Resolves which model compacts: `compactor_model` (override) wins, then the
/// session `model`, then the provider default. This is the second half of the
/// principle "session model compacts, with a configurable same-provider override".
fn resolve_model(compactor_model: Option<String>, model: Option<String>, default: &str) -> String {
    compactor_model.or(model).unwrap_or_else(|| default.to_string())
}

fn build_provider(name: &str, cfg: LlmConfig, client: reqwest::Client) -> DynLlmProvider {
    match name {
        "anthropic" => DynLlmProvider::Anthropic(AnthropicLlmProvider::with_client(cfg, client)),
        // xai / openrouter — OpenAI-compatible Chat Completions.
        _ => DynLlmProvider::OpenAiCompat(OpenAICompatLlmProvider::with_client(cfg, client)),
    }
}

// ── Service ───────────────────────────────────────────────────────────────────

/// Runs the compactor as a NATS service until the connection drops.
///
/// Each incoming request is handled synchronously (one at a time).  For
/// higher throughput, spawn multiple instances behind a NATS queue group.
pub async fn run(nats: Client, state: ServiceState) -> Result<(), async_nats::Error> {
    let mut sub = nats.subscribe(COMPACT_SUBJECT).await?;
    info!(subject = COMPACT_SUBJECT, "compactor service listening");

    while let Some(msg) = sub.next().await {
        let Some(reply) = msg.reply else {
            warn!("received fire-and-forget message on compact subject, ignoring");
            continue;
        };

        let response_bytes = match handle(&state, &msg.payload).await {
            Ok(resp) => serde_json::to_vec(&resp).unwrap_or_default(),
            Err(e) => {
                error!(error = %e, "compaction failed");
                serde_json::to_vec(&ErrorResponse { error: e.to_string() }).unwrap_or_default()
            }
        };

        nats.publish(reply, response_bytes.into()).await.ok();
    }

    Ok(())
}

async fn handle(state: &ServiceState, payload: &[u8]) -> Result<CompactResponse, CompactorError> {
    let req: CompactRequest =
        serde_json::from_slice(payload).map_err(|e| CompactorError::InvalidRequest(e.to_string()))?;

    // Backward-compat: no provider → "anthropic" (the only consumer shape today).
    let provider_name = req.provider.as_deref().unwrap_or("anthropic").to_string();
    let pc = state
        .provider_config(&provider_name)
        .ok_or_else(|| CompactorError::InvalidRequest(format!("provider not configured: {provider_name}")))?;

    // compactor_model overrides model; both absent → provider default.
    let model = resolve_model(req.compactor_model, req.model, &pc.default_model);

    let settings = match req.context_window {
        Some(cw) => CompactionSettings::from_context_window(cw as usize),
        None => state.default_settings.clone(),
    };

    let llm = LlmConfig {
        api_url: pc.api_url.clone(),
        api_key: pc.token.clone(),
        auth_style: pc.auth_style.clone(),
        model,
        max_summary_tokens: state.max_summary_tokens,
    };
    let provider = build_provider(&provider_name, llm, state.client.clone());
    let compactor = Compactor::with_provider(settings, provider);

    let tokens_before = estimate_total_tokens(&req.messages);
    let (messages, kept_count, compacted) =
        compactor.compact_if_needed_counted(req.messages).await?;
    let tokens_after = estimate_total_tokens(&messages);

    Ok(CompactResponse {
        compacted,
        messages,
        tokens_before,
        tokens_after,
        kept_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ServiceState with a dummy anthropic provider. Tiny/empty conversations
    /// don't trigger compaction, so no HTTP call is made.
    fn test_state() -> ServiceState {
        ServiceState {
            client: reqwest::Client::new(),
            default_settings: CompactionSettings::default(),
            max_summary_tokens: 1_000,
            anthropic: Some(ProviderConfig {
                api_url: "http://unused.local/v1/messages".into(),
                token: "tok_test".into(),
                auth_style: AuthStyle::Bearer,
                default_model: "claude-test".into(),
            }),
            xai: None,
            openrouter: None,
            openai: None,
        }
    }

    fn raw_request(json: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json).unwrap()
    }

    #[tokio::test]
    async fn handle_invalid_json_returns_invalid_request_error() {
        let result = handle(&test_state(), b"not valid json {{{").await;
        assert!(
            matches!(result, Err(CompactorError::InvalidRequest(_))),
            "expected InvalidRequest, got {result:?}"
        );
    }

    #[tokio::test]
    async fn handle_tiny_conversation_returns_not_compacted() {
        let req = CompactRequest {
            messages: vec![Message::user("hello"), Message::assistant("world")],
            provider: None,
            model: None,
            compactor_model: None,
            context_window: None,
        };
        let payload = serde_json::to_vec(&req).unwrap();
        let resp = handle(&test_state(), &payload).await.unwrap();

        assert!(!resp.compacted);
        assert_eq!(resp.messages.len(), 2);
        assert!(resp.tokens_before > 0);
        assert_eq!(resp.tokens_before, resp.tokens_after);
        // Gap 2: kept_count == messages.len() when not compacted.
        assert_eq!(resp.kept_count, 2);
    }

    #[tokio::test]
    async fn handle_empty_messages_array_returns_not_compacted() {
        let payload = raw_request(serde_json::json!({ "messages": [] }));
        let resp = handle(&test_state(), &payload).await.unwrap();

        assert!(!resp.compacted);
        assert!(resp.messages.is_empty());
        assert_eq!(resp.tokens_before, 0);
        assert_eq!(resp.tokens_after, 0);
        assert_eq!(resp.kept_count, 0);
    }

    /// Gap 1 backward-compat: an old request shape (only `{messages}`, no
    /// provider/model fields) deserializes and is handled as anthropic.
    #[tokio::test]
    async fn handle_old_request_shape_only_messages_works() {
        let payload = raw_request(serde_json::json!({
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] }
            ]
        }));
        let resp = handle(&test_state(), &payload).await.unwrap();
        assert!(!resp.compacted);
        assert_eq!(resp.messages.len(), 1);
    }

    /// An unconfigured provider yields an InvalidRequest error (not a panic).
    #[tokio::test]
    async fn handle_unconfigured_provider_errors() {
        let payload = raw_request(serde_json::json!({
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }] }],
            "provider": "xai"
        }));
        let result = handle(&test_state(), &payload).await;
        assert!(
            matches!(result, Err(CompactorError::InvalidRequest(_))),
            "expected InvalidRequest for unconfigured provider, got {result:?}"
        );
    }

    // ── resolve_model: the override-precedence principle ──────────────────────

    #[test]
    fn resolve_model_compactor_model_overrides_session_model() {
        // The override must win over the session model.
        let m = resolve_model(Some("haiku".into()), Some("opus".into()), "default-x");
        assert_eq!(m, "haiku");
    }

    #[test]
    fn resolve_model_uses_session_model_when_no_override() {
        // No override → compact with the model the session is using.
        let m = resolve_model(None, Some("opus".into()), "default-x");
        assert_eq!(m, "opus");
    }

    #[test]
    fn resolve_model_falls_back_to_default_when_neither() {
        // Old callers (only {messages}) → provider's configured default.
        let m = resolve_model(None, None, "default-x");
        assert_eq!(m, "default-x");
    }

    // ── build_provider: provider selection ────────────────────────────────────

    fn dummy_cfg() -> LlmConfig {
        LlmConfig {
            api_url: "http://unused".into(),
            api_key: "tok".into(),
            auth_style: AuthStyle::Bearer,
            model: "m".into(),
            max_summary_tokens: 100,
        }
    }

    #[test]
    fn build_provider_selects_anthropic_for_anthropic() {
        let p = build_provider("anthropic", dummy_cfg(), reqwest::Client::new());
        assert!(
            matches!(p, DynLlmProvider::Anthropic(_)),
            "anthropic must use the Anthropic provider"
        );
    }

    #[test]
    fn build_provider_selects_openai_compat_for_xai_and_openrouter() {
        for name in ["xai", "openrouter"] {
            let p = build_provider(name, dummy_cfg(), reqwest::Client::new());
            assert!(
                matches!(p, DynLlmProvider::OpenAiCompat(_)),
                "{name} must use the OpenAI-compatible provider"
            );
        }
    }
}
