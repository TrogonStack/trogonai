mod config;
mod handle;

use async_nats::jetstream;
use config::EnricherConfig;
use futures::StreamExt;
use handle::{EnrichContext, handle_raw_command, handle_raw_event, handle_raw_interaction};
use slack_nats::setup::ensure_slack_stream;
use slack_nats::subscriber::{
    create_raw_command_consumer, create_raw_event_consumer, create_raw_interaction_consumer,
};
use std::sync::Arc;
use std::time::Duration;
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

    let config = EnricherConfig::from_env(&SystemEnv);
    let account_id = config.account_id.as_deref();

    let nats_client = connect(&config.nats, Duration::from_secs(10))
        .await
        .map_err(|e| format!("{:?}", e))?;

    tracing::info!("Setting up JetStream stream...");
    let js = Arc::new(jetstream::new(nats_client));
    ensure_slack_stream(&js).await?;

    tracing::info!("Creating raw event consumers...");
    let event_consumer = create_raw_event_consumer(&js, account_id).await?;
    let interaction_consumer = create_raw_interaction_consumer(&js, account_id).await?;
    let command_consumer = create_raw_command_consumer(&js, account_id).await?;

    let ctx = Arc::new(EnrichContext {
        config,
        http_client: reqwest::Client::new(),
    });

    tracing::info!("Slack enricher running. Press Ctrl+C to stop.");

    let js_event = js.clone();
    let ctx_event = ctx.clone();
    let event_loop = tokio::spawn(async move {
        loop {
            let mut messages = match event_consumer.messages().await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to get event messages stream");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };
            while let Some(Ok(msg)) = messages.next().await {
                let headers = msg.headers.clone().unwrap_or_default();
                handle_raw_event(&*js_event, &ctx_event, &msg.payload, &headers).await;
                if let Err(e) = msg.ack().await {
                    tracing::error!(error = %e, "Failed to ack event message");
                }
            }
        }
    });

    let js_interaction = js.clone();
    let ctx_interaction = ctx.clone();
    let interaction_loop = tokio::spawn(async move {
        loop {
            let mut messages = match interaction_consumer.messages().await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to get interaction messages stream");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };
            while let Some(Ok(msg)) = messages.next().await {
                handle_raw_interaction(&*js_interaction, &ctx_interaction, &msg.payload).await;
                if let Err(e) = msg.ack().await {
                    tracing::error!(error = %e, "Failed to ack interaction message");
                }
            }
        }
    });

    let js_command = js.clone();
    let ctx_command = ctx.clone();
    let command_loop = tokio::spawn(async move {
        loop {
            let mut messages = match command_consumer.messages().await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to get command messages stream");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };
            while let Some(Ok(msg)) = messages.next().await {
                handle_raw_command(&*js_command, &ctx_command, &msg.payload).await;
                if let Err(e) = msg.ack().await {
                    tracing::error!(error = %e, "Failed to ack command message");
                }
            }
        }
    });

    tokio::select! {
        _ = event_loop => tracing::error!("Event loop exited unexpectedly"),
        _ = interaction_loop => tracing::error!("Interaction loop exited unexpectedly"),
        _ = command_loop => tracing::error!("Command loop exited unexpectedly"),
        _ = tokio::signal::ctrl_c() => tracing::info!("Received Ctrl+C, shutting down"),
    }

    Ok(())
}
