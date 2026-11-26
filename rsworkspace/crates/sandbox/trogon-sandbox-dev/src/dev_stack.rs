use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::api_key_provisioning::{generate_api_key, register_api_key, write_api_key_file};
use crate::readiness::{wait_for_broker, wait_for_tcp};
use crate::running_process::{RunningProcess, first_exit, spawn};
use crate::{DevArgs, DevError, DevReady, docker_available, find_binary};

/// Starts the stack described by `args`, calls `on_ready` once every process can serve requests, and supervises it
/// until `shutdown` resolves or one process exits. Every process is stopped before this returns.
///
/// `Ok` means an orderly stop: `shutdown` fired, or a process exited after the stack was ready. A failure during
/// startup is an error.
pub async fn run(
    args: DevArgs,
    shutdown: impl Future<Output = ()> + Send,
    on_ready: impl FnOnce(&DevReady) + Send,
) -> Result<(), DevError> {
    let mut on_ready = Some(on_ready);
    supervise(args, Box::pin(shutdown), &mut |ready| {
        if let Some(on_ready) = on_ready.take() {
            on_ready(ready);
        }
    })
    .await
}

type Shutdown<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

async fn supervise(
    args: DevArgs,
    mut shutdown: Shutdown<'_>,
    on_ready: &mut (dyn FnMut(&DevReady) + Send),
) -> Result<(), DevError> {
    let docker = docker_available().await;
    let worker = args.worker.resolve(
        docker,
        args.agent_binary.is_some(),
        cfg!(any(target_os = "linux", target_os = "macos")),
    );

    let scratch = tempfile::tempdir().map_err(DevError::ScratchDirectory)?;
    let store_dir = args
        .store_dir
        .clone()
        .unwrap_or_else(|| scratch.path().join("jetstream"));
    let workspace_root = scratch.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).map_err(DevError::ScratchDirectory)?;

    let bin_dir = args.bin_dir.clone();
    let plan = args.plan(worker, &store_dir, &workspace_root, |name| {
        find_binary(name, bin_dir.as_deref())
    })?;

    let token = match args.bearer_token()? {
        Some(token) => token,
        None => generate_api_key()?,
    };
    let api_key_file = args
        .api_key_file
        .clone()
        .unwrap_or_else(|| scratch.path().join("api-key"));
    write_api_key_file(&api_key_file, &token)?;

    let mut running = Vec::new();
    let started = start(&args, &plan, &token, &mut running, &mut shutdown).await;
    let outcome = match started {
        Ok(true) => {
            on_ready(&DevReady {
                worker,
                nats_url: plan.nats_url.clone(),
                gateway_http_url: plan.gateway_http_url.clone(),
                gateway_ws_url: plan.gateway_ws_url.clone(),
                tenant: args.tenant.clone(),
                api_key_file,
            });
            tokio::select! {
                () = &mut shutdown => {}
                _ = first_exit(&mut running) => {}
            }
            Ok(())
        }
        Ok(false) => Ok(()),
        Err(error) => Err(error),
    };
    stop_all(&mut running).await;
    outcome
}

/// Spawns the plan into `running`. `Ok(false)` means shutdown arrived before the stack was ready.
async fn start(
    args: &DevArgs,
    plan: &crate::DevPlan,
    token: &trogon_sandbox_auth::BearerToken,
    running: &mut Vec<RunningProcess>,
    shutdown: &mut Shutdown<'_>,
) -> Result<bool, DevError> {
    running.push(spawn(&plan.nats)?);
    let nats_addr = plan.nats_url.trim_start_matches("nats://");
    wait_for_tcp(
        "nats-server",
        nats_addr,
        Duration::from_secs(15),
        "; is the port already in use?",
    )
    .await?;

    register_api_key(&plan.nats_url, &args.tenant, token).await?;

    for spec in &plan.services {
        running.push(spawn(spec)?);
    }
    wait_for_tcp(
        "trogon-sandbox-ws-gateway",
        &args.gateway_listen.to_string(),
        Duration::from_secs(60),
        "",
    )
    .await?;

    tokio::select! {
        result = wait_for_broker(&plan.nats_url, &args.tenant, Duration::from_secs(60)) => result.map(|()| true),
        () = shutdown => Ok(false),
        (name, status) = first_exit(running) => Err(DevError::ExitedDuringStartup { name, status }),
    }
}

async fn stop_all(running: &mut [RunningProcess]) {
    for process in running.iter_mut().rev() {
        let _ = process.child.start_kill();
    }
    for process in running.iter_mut().rev() {
        let _ = process.child.wait().await;
    }
}

/// Installs the shutdown handlers and returns a future that resolves on Ctrl-C everywhere, plus SIGTERM on unix so a
/// supervisor stopping this process still takes the children down with it.
///
/// Must be called inside a tokio runtime.
#[cfg(unix)]
pub fn shutdown_signal() -> Result<impl Future<Output = ()>, DevError> {
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(DevError::SignalHandler)?;
    Ok(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    })
}

/// Installs the shutdown handler and returns a future that resolves on Ctrl-C.
///
/// Must be called inside a tokio runtime.
#[cfg(not(unix))]
pub fn shutdown_signal() -> Result<impl Future<Output = ()>, DevError> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}
