//! NATS request-reply service for context compaction (protobuf wire, M1/M4).

use async_nats::Client;
use futures::StreamExt;
use tracing::{error, info, warn};

use crate::detector::CompactionSettings;
use crate::error::CompactorError;
use crate::summarizer::{AuthStyle, LlmConfig, OpenAICompatLlmProvider};
use crate::tokens::estimate_total_tokens;
use crate::wire::{self, CompactRequest, CompactResponse};
use crate::{AnthropicLlmProvider, Compactor, DynLlmProvider, Message};

/// NATS subject on which the compactor listens for requests.
pub const COMPACT_SUBJECT: &str = "trogon.compactor.compact";

/// Per-provider connection details.
#[derive(Clone)]
pub struct ProviderConfig {
    pub api_url: String,
    pub token: String,
    pub auth_style: AuthStyle,
    pub default_model: String,
}

pub struct ServiceState {
    pub client: reqwest::Client,
    pub default_settings: CompactionSettings,
    pub max_summary_tokens: u32,
    pub anthropic: Option<ProviderConfig>,
    pub xai: Option<ProviderConfig>,
    pub openrouter: Option<ProviderConfig>,
}

impl ServiceState {
    fn provider_config(&self, name: &str) -> Option<&ProviderConfig> {
        match name {
            "anthropic" => self.anthropic.as_ref(),
            "xai" => self.xai.as_ref(),
            "openrouter" => self.openrouter.as_ref(),
            _ => None,
        }
    }
}

fn resolve_model(compactor_model: Option<String>, model: Option<String>, default: &str) -> String {
    compactor_model.or(model).unwrap_or_else(|| default.to_string())
}

fn build_provider(name: &str, cfg: LlmConfig, client: reqwest::Client) -> DynLlmProvider {
    match name {
        "anthropic" => DynLlmProvider::Anthropic(AnthropicLlmProvider::with_client(cfg, client)),
        _ => DynLlmProvider::OpenAiCompat(OpenAICompatLlmProvider::with_client(cfg, client)),
    }
}

fn compactor_provider_name(req: &CompactRequest) -> String {
    req.compactor_provider.clone().unwrap_or_else(|| req.provider.clone())
}

pub async fn run(nats: Client, state: ServiceState) -> Result<(), async_nats::Error> {
    let mut sub = nats.subscribe(COMPACT_SUBJECT).await?;
    info!(subject = COMPACT_SUBJECT, "compactor service listening");

    while let Some(msg) = sub.next().await {
        let Some(reply) = msg.reply else {
            warn!("received fire-and-forget message on compact subject, ignoring");
            continue;
        };

        let response_bytes = match handle(&state, &msg.payload).await {
            Ok(resp) => wire::encode_response(&resp),
            Err(e) => {
                error!(error = %e, "compaction failed");
                wire::encode_error(&e)
            }
        };

        nats.publish(reply, response_bytes.into()).await.ok();
    }

    Ok(())
}

async fn handle(state: &ServiceState, payload: &[u8]) -> Result<CompactResponse, CompactorError> {
    let req = wire::decode_request(payload)?;

    let session_provider = if req.provider.is_empty() {
        "anthropic".to_string()
    } else {
        req.provider.clone()
    };

    let compactor_provider = compactor_provider_name(&req);
    let session_model = resolve_model(
        None,
        req.model.clone(),
        state
            .provider_config(&session_provider)
            .map(|p| p.default_model.as_str())
            .unwrap_or(""),
    );
    let chosen_model = resolve_model(
        req.compactor_model.clone(),
        req.model.clone(),
        state
            .provider_config(&compactor_provider)
            .map(|p| p.default_model.as_str())
            .unwrap_or(""),
    );

    let settings = match req.context_window {
        Some(cw) => CompactionSettings::from_context_window(cw as usize),
        None => state.default_settings.clone(),
    };

    let tokens_before = estimate_total_tokens(&req.messages);

    // M4: try chosen model first, fallback to session model on any error.
    match compact_with_provider(
        state,
        &compactor_provider,
        &chosen_model,
        settings.clone(),
        req.messages.clone(),
    )
    .await
    {
        Ok((messages, kept_count)) => {
            let tokens_after = estimate_total_tokens(&messages);
            Ok(CompactResponse {
                compacted: tokens_after < tokens_before,
                messages,
                tokens_before,
                tokens_after,
                kept_count,
                fallback_model: None,
            })
        }
        Err(primary_err) => {
            warn!(
                error = %primary_err,
                compactor_provider,
                chosen_model,
                "compaction failed with chosen model; falling back to session model"
            );
            let (messages, kept_count) =
                compact_with_provider(state, &session_provider, &session_model, settings, req.messages).await?;
            let tokens_after = estimate_total_tokens(&messages);
            Ok(CompactResponse {
                compacted: tokens_after < tokens_before,
                messages,
                tokens_before,
                tokens_after,
                kept_count,
                fallback_model: Some(session_model),
            })
        }
    }
}

async fn compact_with_provider(
    state: &ServiceState,
    provider_name: &str,
    model: &str,
    settings: CompactionSettings,
    messages: Vec<Message>,
) -> Result<(Vec<Message>, usize), CompactorError> {
    let pc = state
        .provider_config(provider_name)
        .ok_or_else(|| CompactorError::InvalidRequest(format!("provider not configured: {provider_name}")))?;

    let llm = LlmConfig {
        api_url: pc.api_url.clone(),
        api_key: pc.token.clone(),
        auth_style: pc.auth_style.clone(),
        model: model.to_string(),
        max_summary_tokens: state.max_summary_tokens,
    };
    let provider = build_provider(provider_name, llm, state.client.clone());
    let compactor = Compactor::with_provider(settings, provider);
    compactor.compact_if_needed_counted(messages).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::Message as _;
    use trogonai_compactor_proto::CompactRequest as ProtoRequest;

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
        }
    }

    #[tokio::test]
    async fn handle_tiny_conversation_returns_not_compacted() {
        let proto = ProtoRequest {
            messages: vec![wire::message_to_proto(&Message::user("hello"))],
            provider: "anthropic".into(),
            model: String::new(),
            context_window: 0,
            compactor_provider: None,
            compactor_model: None,
            __buffa_unknown_fields: Default::default(),
        };
        let payload = proto.encode_to_vec();
        let resp = handle(&test_state(), &payload).await.unwrap();
        assert!(!resp.compacted);
        assert_eq!(resp.messages.len(), 1);
        assert_eq!(resp.kept_count, 1);
        assert!(resp.fallback_model.is_none());
    }

    #[tokio::test]
    async fn compactor_provider_selects_provider_config() {
        let mut state = test_state();
        state.xai = Some(ProviderConfig {
            api_url: "http://unused.local/v1/chat/completions".into(),
            token: "tok_xai".into(),
            auth_style: AuthStyle::Bearer,
            default_model: "grok-test".into(),
        });

        let req = CompactRequest {
            messages: vec![Message::user("hi")],
            provider: "anthropic".into(),
            model: Some("claude-test".into()),
            context_window: None,
            compactor_provider: Some("xai".into()),
            compactor_model: Some("grok-test".into()),
        };
        assert_eq!(compactor_provider_name(&req), "xai");
    }

    #[test]
    fn compactor_provider_absent_falls_back_to_session_provider() {
        let req = CompactRequest {
            messages: vec![],
            provider: "anthropic".into(),
            model: None,
            context_window: None,
            compactor_provider: None,
            compactor_model: None,
        };
        assert_eq!(compactor_provider_name(&req), "anthropic");
    }

    #[test]
    fn resolve_model_compactor_model_overrides_session_model() {
        let m = resolve_model(Some("haiku".into()), Some("opus".into()), "default-x");
        assert_eq!(m, "haiku");
    }

    #[tokio::test]
    async fn m4_fallback_to_session_model_on_primary_error() {
        let proto = ProtoRequest {
            messages: vec![wire::message_to_proto(&Message::user("hello"))],
            provider: "anthropic".into(),
            model: "claude-test".into(),
            context_window: 0,
            compactor_provider: Some("xai".into()),
            compactor_model: Some("grok-test".into()),
            __buffa_unknown_fields: Default::default(),
        };
        let payload = proto.encode_to_vec();
        let resp = handle(&test_state(), &payload).await.unwrap();
        assert_eq!(resp.fallback_model.as_deref(), Some("claude-test"));
        assert_eq!(resp.messages.len(), 1);
    }

    #[test]
    fn protobuf_compat_missing_new_fields_defaults_to_session_provider() {
        let proto = ProtoRequest {
            messages: vec![],
            provider: "anthropic".into(),
            model: "opus".into(),
            context_window: 0,
            compactor_provider: None,
            compactor_model: None,
            __buffa_unknown_fields: Default::default(),
        };
        let bytes = proto.encode_to_vec();
        let req = wire::decode_request(&bytes).unwrap();
        assert!(req.compactor_provider.is_none());
        assert_eq!(req.provider, "anthropic");
    }
}
