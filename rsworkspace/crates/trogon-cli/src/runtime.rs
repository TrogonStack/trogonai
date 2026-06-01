//! Interactive runtime: `LocalSet` + ACP `client::run` + REPL.
//!
//! `Bridge` and `CrossRunnerSwitcher` are `!Send` — must run inside `LocalSet`.

use acp_nats::Config;
use agent_client_protocol::SessionNotification;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::client_supervisor::AcpClientSupervisor;
use crate::fs::Fs;
use crate::repl;
use crate::session::SessionFactory;
use crate::tui_client::{ActiveClientState, TuiClient};

pub use crate::tui_client::PermissionCoordinator;
use crate::RunnerSwitcher;
use trogon_nats::jetstream::NatsJetStreamClient;

/// Resolve the startup session mode from the CLI flags.
///
/// `--dangerously-skip-permissions` and `--plan` select fixed modes (and are
/// mutually exclusive at the clap layer). Otherwise the mode is taken from
/// `TROGON_MODE`, falling back to `"default"`. This mirrors how the REPL keeps
/// its tracked `session_mode` in sync after `/clear` and `/model` switches.
pub fn startup_mode(skip_permissions: bool, plan: bool) -> String {
    if skip_permissions {
        "bypassPermissions".to_string()
    } else if plan {
        "plan".to_string()
    } else {
        std::env::var("TROGON_MODE").unwrap_or_else(|_| "default".into())
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_interactive<SF, F, SW, RS>(
    factory: SF,
    prefix: &str,
    cwd: PathBuf,
    fs: F,
    switcher: SW,
    registry: trogon_registry::Registry<RS>,
    nats: async_nats::Client,
    _config: Config,
    nats_url: String,
    stream: bool,
    resume: Option<crate::session_store::SessionEntry>,
    skip_permissions: bool,
    plan: bool,
) -> anyhow::Result<()>
where
    SF: SessionFactory,
    F: Fs,
    SW: RunnerSwitcher,
    RS: trogon_registry::RegistryStore,
{
    let local = tokio::task::LocalSet::new();
    local
        .run_until(run_interactive_inner(
            factory, prefix, cwd, fs, switcher, registry, nats, nats_url, stream, resume,
            skip_permissions, plan,
        ))
        .await
}

#[allow(clippy::too_many_arguments)]
async fn run_interactive_inner<SF, F, SW, RS>(
    factory: SF,
    prefix: &str,
    cwd: PathBuf,
    fs: F,
    switcher: SW,
    registry: trogon_registry::Registry<RS>,
    nats: async_nats::Client,
    nats_url: String,
    stream: bool,
    resume: Option<crate::session_store::SessionEntry>,
    skip_permissions: bool,
    plan: bool,
) -> anyhow::Result<()>
where
    SF: SessionFactory,
    F: Fs,
    SW: RunnerSwitcher,
    RS: trogon_registry::RegistryStore,
{
    let (notification_tx, mut notification_rx) = mpsc::channel::<SessionNotification>(64);
    tokio::task::spawn_local(async move {
        while notification_rx.recv().await.is_some() {}
    });

    let js = async_nats::jetstream::new(nats.clone());
    let js_client = NatsJetStreamClient::new(js);

    let client_state = Arc::new(Mutex::new(ActiveClientState {
        session_id: None,
        prefix: prefix.to_string(),
        allowed_tools: Vec::new(),
    }));
    let permission_coordinator = PermissionCoordinator::new();
    let tui_client = Rc::new(TuiClient::new(
        client_state.clone(),
        permission_coordinator.clone(),
    ));

    let supervisor = Rc::new(AcpClientSupervisor::new(
        client_state,
        tui_client,
        nats,
        js_client,
        nats_url,
        notification_tx,
        prefix,
    )?);

    repl::run(
        factory,
        prefix,
        cwd,
        fs,
        switcher,
        registry,
        Some(supervisor),
        permission_coordinator,
        stream,
        resume,
        skip_permissions,
        plan,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::startup_mode;

    #[test]
    fn skip_permissions_selects_bypass() {
        // `--dangerously-skip-permissions` always wins and does not read TROGON_MODE.
        assert_eq!(startup_mode(true, false), "bypassPermissions");
    }

    #[test]
    fn plan_selects_plan_mode() {
        // `--plan` selects plan mode regardless of TROGON_MODE.
        assert_eq!(startup_mode(false, true), "plan");
    }
}
