use std::sync::Arc;

use prometheus::Registry;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_protocol::DispatchSigner;
use trogon_std::env::SystemEnv;

use crate::{ServeError, SnapshotBuilder, SnapshotBuilderArgs, SnapshotBuilderMetrics};

/// Runs the builder described by `args` until `cancel` fires, reporting on `registry`.
pub async fn serve(
    args: &SnapshotBuilderArgs,
    registry: &Registry,
    cancel: CancellationToken,
) -> Result<(), ServeError> {
    let dispatch_signer = match DispatchSigner::from_env(&SystemEnv) {
        Ok(signer) => signer.map(Arc::new),
        Err(error) => {
            warn!(%error, "dispatch signing key is unusable; worker export requests go unsigned");
            None
        }
    };
    let config = args.builder_config(dispatch_signer)?;
    let metrics = SnapshotBuilderMetrics::register(registry)?;
    let builder = SnapshotBuilder::connect(config, metrics).await?;
    info!(nats_url = %args.nats_url, "snapshot builder starting");
    Ok(builder.run(cancel).await?)
}
