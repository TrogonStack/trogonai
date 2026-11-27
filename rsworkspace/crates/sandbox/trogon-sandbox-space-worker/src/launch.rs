use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, nats_connect, register_state_dir};
use trogon_std::env::SystemEnv;

use crate::{
    LaunchError, MaxLeases, SpaceApi, SpaceConfig, SpaceConfigParts, SpaceOrigin, SpaceProvider, SpaceTemplate,
    SpaceToken, SpaceWorker, WorkerArgs,
};

/// Runs the worker described by `args` until interrupted.
pub async fn launch(args: WorkerArgs) -> Result<(), LaunchError> {
    let mode = AuthorityMode::from_env(&SystemEnv)?;
    let stop = CancellationToken::new();
    let interrupt = stop.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        interrupt.cancel();
    });
    launch_until(args, mode, stop).await
}

/// Runs the worker described by `args` until `stop` fires.
pub async fn launch_until(args: WorkerArgs, mode: AuthorityMode, stop: CancellationToken) -> Result<(), LaunchError> {
    let control = SpaceOrigin::parse(args.endpoint)?;
    let data = SpaceOrigin::parse(args.data_plane_url)?;
    let token = SpaceToken::parse(&args.token)?;
    let api = SpaceApi::new(control, data, token)?;
    let config = SpaceConfig::new(SpaceConfigParts {
        template: SpaceTemplate::parse(args.template)?,
        state_dir: args.state_dir,
        boot_timeout: Duration::from_secs(args.boot_timeout_seconds),
        operation_timeout: Duration::from_secs(args.operation_timeout_seconds),
        poll_interval: Duration::from_secs(1),
        max_leases: MaxLeases::parse(args.max_leases)?,
        command_grace: Duration::from_secs(15),
        file_read_timeout: Duration::from_secs(300),
    })?;
    register_state_dir(&config.state_dir);
    let provider = Arc::new(SpaceProvider::new(api, config)?);
    let reclaimed = provider.recover().await?;
    tracing::info!(reclaimed, "reclaimed sandboxes from an earlier run");
    if args
        .worker_id
        .is_some_and(|configured| configured != provider.dispatch_ledger().worker_id().as_str())
    {
        return Err(LaunchError::WorkerIdMismatch);
    }
    let client = nats_connect::connect(&args.nats_url).await?;
    Ok(SpaceWorker::new(client, provider, AuthorityConfig::new(mode))
        .run(stop)
        .await?)
}
