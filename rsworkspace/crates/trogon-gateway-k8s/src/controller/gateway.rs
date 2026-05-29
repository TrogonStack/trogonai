use std::sync::Arc;

use kube::runtime::controller::Action;
use kube::Api;
use kube::Resource;

use crate::controller::policy::{publish_bytes, run_controller, watcher_config, ReconcileError};
use crate::controller::ControllerContext;
use crate::crd::Gateway;
use crate::nats::ConfigKvError;
use crate::projection::{project_gateway, ProjectionError};

pub async fn run_gateway_controller(
    ctx: Arc<ControllerContext>,
    watch_namespace: Option<String>,
) -> Result<(), ReconcileError> {
    let api = match &watch_namespace {
        Some(namespace) => Api::<Gateway>::namespaced(ctx.client.clone(), namespace),
        None => Api::<Gateway>::all(ctx.client.clone()),
    };
    run_controller(api, watcher_config(), ctx, reconcile_gateway).await
}

async fn reconcile_gateway(
    resource: Arc<Gateway>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ConfigKvError> {
    if resource.meta().deletion_timestamp.is_some() {
        let projection = project_gateway(&resource).map_err(map_projection_error)?;
        ctx.kv.delete_key(&projection.key).await?;
        return Ok(Action::await_change());
    }

    let projection = project_gateway(&resource).map_err(map_projection_error)?;
    let bytes = serde_json::to_vec(&projection.value).map_err(|error| {
        ConfigKvError::Put(format!("gateway json: {error}"))
    })?;
    publish_bytes(
        &ctx.kv,
        &projection.key,
        &bytes,
        &projection.content_hash,
    )
    .await?;
    Ok(Action::await_change())
}

fn map_projection_error(error: ProjectionError) -> ConfigKvError {
    ConfigKvError::Put(error.to_string())
}
