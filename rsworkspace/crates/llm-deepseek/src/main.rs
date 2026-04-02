mod config;
mod worker;

use config::LlmDeepSeekConfig;
use futures::StreamExt;
use llm_types::{llm_subject_for_account, LlmPromptRequest, LLM_REQUEST_DEEPSEEK};
use std::sync::Arc;
use trogon_nats::connect;
use trogon_std::env::SystemEnv;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let config = LlmDeepSeekConfig::from_env(&SystemEnv);
    let account_id = config.account_id.as_deref();

    tracing::info!(
        model = %config.default_model,
        max_tokens = config.default_max_tokens,
        "Starting llm-deepseek worker"
    );

    let nats_client = Arc::new(
        connect(&config.nats)
            .await
            .map_err(|e| format!("NATS connect failed: {e:?}"))?,
    );

    let subject = llm_subject_for_account(LLM_REQUEST_DEEPSEEK, account_id);

    let mut sub = nats_client
        .queue_subscribe(subject.clone(), "llm-deepseek-workers".to_string())
        .await?;

    tracing::info!(subject = %subject, "Subscribed — waiting for LLM requests");

    let api_key = Arc::new(config.api_key);
    let default_model = Arc::new(config.default_model);
    let default_max_tokens = config.default_max_tokens;
    let retry_attempts = config.retry_attempts;
    let http = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?,
    );

    tokio::select! {
        _ = async {
            while let Some(msg) = sub.next().await {
                match serde_json::from_slice::<LlmPromptRequest>(&msg.payload) {
                    Ok(req) => {
                        tracing::debug!(
                            model = req.model.as_deref().unwrap_or(&default_model),
                            inbox = %req.stream_inbox,
                            "Handling LLM request"
                        );
                        let nats = nats_client.clone();
                        let http = http.clone();
                        let api_key = api_key.clone();
                        let default_model = default_model.clone();
                        tokio::spawn(async move {
                            worker::handle_request(
                                &nats,
                                &http,
                                &api_key,
                                &default_model,
                                default_max_tokens,
                                retry_attempts,
                                req,
                            )
                            .await;
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to deserialize LlmPromptRequest");
                    }
                }
            }
        } => {
            tracing::warn!("NATS subscription closed");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down");
        }
    }

    Ok(())
}
