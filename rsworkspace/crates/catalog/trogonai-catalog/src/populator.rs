//! Fetches `/models` from each provider via secret-proxy and writes NATS KV snapshots.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use buffa::{EnumValue, Message as _};
use reqwest::Client;
use tracing::{info, warn};
use trogonai_catalog_client::{CATALOG_KEY, CatalogEntry, PROVIDERS_KEY};
use trogonai_catalog_proto::{
    CatalogEntry as ProtoEntry, CatalogSnapshot as ProtoSnapshot, ModelModality, ProviderSnapshot,
};

const PROVIDERS: &[&str] = &["anthropic", "xai", "openrouter"];

/// Lateral table for context windows missing from `/models` responses.
fn static_context_window(provider: &str, model_id: &str) -> Option<u64> {
    match (provider, model_id) {
        ("anthropic", id) if id.contains("claude") => Some(200_000),
        ("xai", id) if id.contains("grok") => Some(131_072),
        ("openrouter", _) => Some(128_000),
        _ => None,
    }
}

pub struct Populator {
    pub client: Client,
    pub proxy_url: String,
    pub store: async_nats::jetstream::kv::Store,
    pub nats: async_nats::Client,
    pub nats_prefix: String,
}

impl Populator {
    pub async fn refresh_all(&self) -> Result<(), PopulatorError> {
        let providers = self.discover_providers().await?;
        self.write_providers(&providers).await?;

        let mut entries = Vec::new();
        for provider in &providers {
            match self.fetch_provider_models(provider).await {
                Ok(mut models) => entries.append(&mut models),
                Err(e) => warn!(provider, error = %e, "failed to fetch models"),
            }
        }

        self.write_catalog(&entries).await?;
        info!(count = entries.len(), providers = ?providers, "catalog refreshed");
        Ok(())
    }

    async fn discover_providers(&self) -> Result<Vec<String>, PopulatorError> {
        let subject = format!("{}.proxy.providers", self.nats_prefix);
        if let Ok(Ok(reply)) = tokio::time::timeout(Duration::from_secs(2), self.nats.request(subject, "".into())).await
            && let Ok(proto) = ProviderSnapshot::decode_from_slice(&reply.payload)
            && !proto.providers.is_empty()
        {
            return Ok(proto.providers);
        }

        // Fall back to env probe when introspection is unavailable.
        let mut found = Vec::new();
        for provider in PROVIDERS {
            let token_env = match *provider {
                "anthropic" => "ANTHROPIC_TOKEN",
                "xai" => "XAI_API_KEY",
                "openrouter" => "OPENROUTER_API_KEY",
                _ => continue,
            };
            if std::env::var(token_env).is_ok() {
                found.push(provider.to_string());
            }
        }
        Ok(found)
    }

    async fn fetch_provider_models(&self, provider: &str) -> Result<Vec<CatalogEntry>, PopulatorError> {
        let url = match provider {
            "anthropic" => format!("{}/anthropic/v1/models", self.proxy_url.trim_end_matches('/')),
            "xai" => format!("{}/xai/v1/models", self.proxy_url.trim_end_matches('/')),
            "openrouter" => format!("{}/openrouter/v1/models", self.proxy_url.trim_end_matches('/')),
            _ => return Ok(Vec::new()),
        };

        let token_env = match provider {
            "anthropic" => "ANTHROPIC_TOKEN",
            "xai" => "XAI_API_KEY",
            "openrouter" => "OPENROUTER_API_KEY",
            _ => return Ok(Vec::new()),
        };
        let token = std::env::var(token_env).map_err(|_| PopulatorError::MissingToken {
            provider: provider.to_string(),
        })?;

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| PopulatorError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(PopulatorError::Http(format!("GET {url} returned {}", resp.status())));
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| PopulatorError::Http(e.to_string()))?;

        parse_models_response(provider, &body)
    }

    async fn write_catalog(&self, entries: &[CatalogEntry]) -> Result<(), PopulatorError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let proto = ProtoSnapshot {
            entries: entries
                .iter()
                .map(|e| ProtoEntry {
                    model_id: e.model_id.clone(),
                    provider: e.provider.clone(),
                    context_window: e.context_window,
                    max_output: e.max_output,
                    modality: EnumValue::Known(e.modality),
                    __buffa_unknown_fields: Default::default(),
                })
                .collect(),
            fetched_at_unix_secs: now,
            __buffa_unknown_fields: Default::default(),
        };
        self.store
            .put(CATALOG_KEY, proto.encode_to_vec().into())
            .await
            .map_err(|e| PopulatorError::Kv(e.to_string()))?;
        Ok(())
    }

    async fn write_providers(&self, providers: &[String]) -> Result<(), PopulatorError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let proto = ProviderSnapshot {
            providers: providers.to_vec(),
            fetched_at_unix_secs: now,
            __buffa_unknown_fields: Default::default(),
        };
        self.store
            .put(PROVIDERS_KEY, proto.encode_to_vec().into())
            .await
            .map_err(|e| PopulatorError::Kv(e.to_string()))?;
        Ok(())
    }
}

fn parse_models_response(provider: &str, body: &serde_json::Value) -> Result<Vec<CatalogEntry>, PopulatorError> {
    let data = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::new();
    for item in data {
        let model_id = item
            .get("id")
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if model_id.is_empty() {
            continue;
        }

        let modality = infer_modality(&model_id, &item);
        if !matches!(
            modality,
            ModelModality::TEXT | ModelModality::MODEL_MODALITY_UNSPECIFIED
        ) {
            continue;
        }

        let context_window = item
            .get("context_window")
            .or_else(|| item.get("context_length"))
            .and_then(|v| v.as_u64())
            .or_else(|| static_context_window(provider, &model_id))
            .unwrap_or(128_000);

        let max_output = item
            .get("max_output_tokens")
            .or_else(|| item.get("max_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(8192) as u32;

        out.push(CatalogEntry {
            model_id,
            provider: provider.to_string(),
            context_window,
            max_output,
            modality,
        });
    }
    Ok(out)
}

fn infer_modality(model_id: &str, item: &serde_json::Value) -> ModelModality {
    let id_lower = model_id.to_lowercase();
    if id_lower.contains("embed") {
        return ModelModality::EMBEDDING;
    }
    if id_lower.contains("image") || id_lower.contains("dall") {
        return ModelModality::IMAGE;
    }
    if id_lower.contains("moderat") {
        return ModelModality::MODERATION;
    }
    if id_lower.contains("classif") {
        return ModelModality::CLASSIFICATION;
    }
    if let Some(kind) = item.get("type").and_then(|v| v.as_str()) {
        match kind {
            "embedding" => return ModelModality::EMBEDDING,
            "image" => return ModelModality::IMAGE,
            _ => {}
        }
    }
    ModelModality::TEXT
}

#[derive(Debug, thiserror::Error)]
pub enum PopulatorError {
    #[error("missing token for provider {provider}")]
    MissingToken { provider: String },
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("KV error: {0}")]
    Kv(String),
}
