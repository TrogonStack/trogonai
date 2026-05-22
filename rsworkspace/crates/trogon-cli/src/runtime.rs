//! Interactive runtime: `LocalSet` + ACP `client::run` + REPL.
//!
//! `Bridge` and `CrossRunnerSwitcher` are `!Send` — must run inside `LocalSet`.

use acp_nats::{agent::Bridge, client, Config, StdJsonSerialize};
use agent_client_protocol::SessionNotification;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::fs::Fs;
use crate::repl;
use crate::session::SessionFactory;
use crate::tui_client::{ActiveClientState, TuiClient};
use crate::RunnerSwitcher;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_std::time::SystemClock;

pub async fn run_interactive<SF, F, SW>(
    factory: SF,
    prefix: &str,
    cwd: PathBuf,
    fs: F,
    switcher: SW,
    nats: async_nats::Client,
    config: Config,
) -> anyhow::Result<()>
where
    SF: SessionFactory,
    F: Fs,
    SW: RunnerSwitcher,
{
    let local = tokio::task::LocalSet::new();
    local
        .run_until(run_interactive_inner(
            factory, prefix, cwd, fs, switcher, nats, config,
        ))
        .await
}

async fn run_interactive_inner<SF, F, SW>(
    factory: SF,
    prefix: &str,
    cwd: PathBuf,
    fs: F,
    switcher: SW,
    nats: async_nats::Client,
    config: Config,
) -> anyhow::Result<()>
where
    SF: SessionFactory,
    F: Fs,
    SW: RunnerSwitcher,
{
    let (notification_tx, mut notification_rx) = mpsc::channel::<SessionNotification>(64);
    tokio::task::spawn_local(async move {
        while notification_rx.recv().await.is_some() {}
    });

    let js = async_nats::jetstream::new(nats.clone());
    let js_client = NatsJetStreamClient::new(js);
    let meter = opentelemetry::global::meter("trogon-cli");
    let bridge = Rc::new(Bridge::new(
        nats.clone(),
        js_client,
        SystemClock,
        &meter,
        config,
        notification_tx,
    ));

    let client_state = Arc::new(Mutex::new(ActiveClientState {
        session_id: None,
        prefix: prefix.to_string(),
        allowed_tools: Vec::new(),
    }));
    let tui_client = Rc::new(TuiClient::new(client_state.clone()));

    let nats_client = nats.clone();
    let client_task = tokio::task::spawn_local(async move {
        client::run(nats_client, tui_client, bridge, StdJsonSerialize).await;
    });

    let repl_result = repl::run(factory, prefix, cwd, fs, switcher).await;
    client_task.abort();
    repl_result
}
