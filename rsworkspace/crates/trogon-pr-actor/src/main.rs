mod config;

use std::time::Duration;

use acp_telemetry::ServiceName;
use clap::Parser;
use tracing::info;
use trogon_actor::{ActorRuntime, host::ActorHost, inbox::provision_actor_inbox, provision_state};
use trogon_nats::{connect, jetstream::NatsJetStreamClient};
use trogon_registry::{AgentCapability, Registry, provision as provision_registry};
use trogon_service_config::{NatsArgs, RuntimeConfigArgs, load_config, resolve_nats};
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;
use trogon_transcript::publisher::NatsTranscriptPublisher;

use reqwest;
use trogon_pr_actor::actor::PrActor;

const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser, Debug)]
#[command(name = "trogon-pr-actor", about = "TrogonStack PR Entity Actor")]
struct Cli {
    #[command(flatten)]
    pub runtime: RuntimeConfigArgs,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    acp_telemetry::init_logger(ServiceName::TrogonPrActor, "pr-actor", &SystemEnv, &SystemFs);

    info!("trogon-pr-actor starting");

    // ── Load config ───────────────────────────────────────────────────────────
    let cfg: config::PrActorConfig = load_config(cli.runtime.config.as_deref())?;

    // ── Connect to NATS ───────────────────────────────────────────────────────
    let nats_config = resolve_nats(&cfg.nats, &NatsArgs::default());
    let nats = connect(&nats_config, NATS_CONNECT_TIMEOUT).await?;
    let js = async_nats::jetstream::new(nats.clone());

    info!(nats_url = %cfg.nats.url, "NATS connected");

    // ── Provision required infrastructure ────────────────────────────────────
    let state_store = provision_state(&js).await?;
    info!("ACTOR_STATE KV bucket ready");

    let registry_store = provision_registry(&js).await.map_err(|e| e.to_string())?;
    info!("AGENT_REGISTRY KV bucket ready");

    let js_client = NatsJetStreamClient::new(js.clone());
    provision_actor_inbox(&js_client).await?;
    info!("ACTOR_INBOX stream ready");

    // ── Build runtime and host ────────────────────────────────────────────────
    let publisher = NatsTranscriptPublisher::new(js);
    let registry = Registry::new(registry_store);
    let runtime = ActorRuntime::new(state_store, publisher, nats.clone(), registry);

    let capability = AgentCapability::new(
        "PrActor",
        ["code_review", "pull_request"],
        "actors.pr.>",
    );

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let actor = PrActor::new(
        http,
        cfg.llm_api_url,
        cfg.llm_api_key,
        cfg.llm_model,
        cfg.github_token,
    );
    let host = ActorHost::new(runtime, actor, capability);

    info!("PR actor registered, listening on actors.pr.>");

    // ── Run until SIGTERM / SIGINT ────────────────────────────────────────────
    tokio::select! {
        result = host.run_durable(&js_client) => {
            result?;
        }
        _ = acp_telemetry::signal::shutdown_signal() => {
            info!("shutdown signal received");
        }
    }

    info!("trogon-pr-actor stopped");
    Ok(())
}
