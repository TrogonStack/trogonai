mod config;

use std::time::Duration;

use acp_telemetry::ServiceName;
use clap::Parser;
use tracing::info;
use trogon_nats::connect;
use trogon_service_config::{NatsArgs, RuntimeConfigArgs, load_config, resolve_nats};
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;

use trogon_actor::inbox::provision_actor_inbox;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_registry::provision as provision_registry;
use trogon_router::{
    llm::{LLM_REQUEST_TIMEOUT, LlmConfig, OpenAiCompatClient},
    router::Router,
    unroutable::{UNROUTABLE_SUBJECT_PREFIX, provision_unroutable_stream},
};
use trogon_transcript::{publisher::NatsTranscriptPublisher, store::TranscriptStore};

const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser, Debug)]
#[command(name = "trogon-router", about = "TrogonStack dynamic-agent router")]
struct Cli {
    #[command(flatten)]
    pub runtime: RuntimeConfigArgs,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    acp_telemetry::init_logger(ServiceName::TrogonRouter, "router", &SystemEnv, &SystemFs);

    info!("trogon-router starting");

    // ── Load config ───────────────────────────────────────────────────────────
    let cfg: config::RouterConfig = load_config(cli.runtime.config.as_deref())?;

    if cfg.llm_api_key.is_empty() {
        return Err("ROUTER_LLM_API_KEY is not set".into());
    }

    // ── Connect to NATS ───────────────────────────────────────────────────────
    let nats_config = resolve_nats(&cfg.nats, &NatsArgs::default());
    let nats = connect(&nats_config, NATS_CONNECT_TIMEOUT).await?;
    let js = async_nats::jetstream::new(nats.clone());

    info!(
        nats_url = %cfg.nats.url,
        events_subject = %cfg.events_subject,
        llm_model = %cfg.llm_model,
        "NATS connected"
    );

    // ── Provision required infrastructure ────────────────────────────────────
    TranscriptStore::new(js.clone()).provision().await?;
    info!("TRANSCRIPTS stream ready");

    let registry_store = provision_registry(&js).await?;
    info!("AGENT_REGISTRY KV bucket ready");

    provision_unroutable_stream(&js)
        .await
        .map_err(|e| e.as_str().to_string())?;
    info!("UNROUTABLE_EVENTS stream ready");

    let js_client = NatsJetStreamClient::new(js.clone());
    provision_actor_inbox(&js_client).await?;
    info!("ACTOR_INBOX stream ready");

    // ── Build router ──────────────────────────────────────────────────────────
    let http = reqwest::Client::builder()
        .timeout(LLM_REQUEST_TIMEOUT)
        .build()?;
    let llm = OpenAiCompatClient::new(
        http,
        LlmConfig {
            api_url: cfg.llm_api_url.clone(),
            api_key: cfg.llm_api_key.clone(),
            model: cfg.llm_model.clone(),
        },
    );
    let registry = trogon_registry::Registry::new(registry_store);
    let publisher = NatsTranscriptPublisher::new(js);
    let router =
        Router::new(llm, registry, publisher, nats, js_client).with_dlq(UNROUTABLE_SUBJECT_PREFIX);

    // ── Run ───────────────────────────────────────────────────────────────────
    info!(subject = %cfg.events_subject, "router listening");

    tokio::select! {
        result = router.run(&cfg.events_subject) => {
            result?;
        }
        _ = acp_telemetry::signal::shutdown_signal() => {
            info!("shutdown signal received");
        }
    }

    info!("trogon-router stopped");
    Ok(())
}
