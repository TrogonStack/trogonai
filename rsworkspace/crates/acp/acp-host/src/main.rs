#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! ACP client host: runs any ACP agent CLI as a child process and serves it on
//! the `acp-nats` subjects, so every NATS-side client (channel bridges, the
//! stdio and WebSocket bridges) can reach it. See
//! `docs/research/acp/decision-record.md` ("The gap") and
//! `docs/research/acp/host-role-and-invocation.md` for the role this fills.
//!
//! Which agent runs is configuration only (`ACP_HOST_AGENT`), so swapping
//! Claude Code for Codex, Gemini, Pi, or fx is a restart with a different
//! command, not a code change.

mod agent_launch;
mod client_claim;
mod client_handshake;
mod config;
mod constants;
mod initialized_agent;
mod restart_policy;
mod serving;
mod session_routed_client;
mod streams;
mod supervisor;

use acp_nats::boundary::ConnectionAgent;
use acp_nats::nats;
use client_claim::ClientClaim;
use config::HostConfig;
use initialized_agent::InitializedAgent;
use restart_policy::RestartPolicy;
use session_routed_client::SessionRoutedClient;
use std::sync::Arc;
use tracing::{error, info};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;
use trogon_std::signal::shutdown_signal;
use trogon_telemetry::{ResourceAttribute, ServiceName};

#[tokio::main(flavor = "current_thread")]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() -> anyhow::Result<()> {
    let config = HostConfig::from_env(&SystemEnv)?;
    trogon_telemetry::init_logger(
        ServiceName::AcpHost,
        [ResourceAttribute::acp_prefix(config.acp.acp_prefix())],
        &SystemEnv,
        &SystemFs,
    );
    let acp = acp_nats::apply_timeout_overrides(config.acp, &SystemEnv);
    info!(agent = ?config.agent, prefix = acp.acp_prefix(), "ACP host starting");

    let nats_client = nats::connect(acp.nats(), acp_nats::nats_connect_timeout(&SystemEnv)).await?;
    let js_client = NatsJetStreamClient::new(async_nats::jetstream::new(nats_client.clone()));
    streams::provision(&js_client, acp.acp_prefix_ref()).await?;

    let client = Arc::new(SessionRoutedClient::new(
        nats_client.clone(),
        acp.acp_prefix_ref().clone(),
        acp.operation_timeout(),
    ));
    let prefix = acp.acp_prefix_ref().clone();
    let serve = move |agent: InitializedAgent<ConnectionAgent>| {
        serving::serve(agent, nats_client.clone(), js_client.clone(), prefix.clone())
    };

    // The NATS agent side runs `!Send` tasks on the current thread.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::select! {
                _ = supervisor::supervise(&config.agent, client, Arc::new(ClientClaim::default()), serve, RestartPolicy::DEFAULT) => {}
                () = shutdown_signal() => info!("ACP host shutting down (signal received)"),
            }
        })
        .await;

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }
    Ok(())
}
