use std::sync::Arc;

use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::Resource;
use kube::ResourceExt;

use crate::controller::policy::{publish_bytes, run_controller, watcher_config, ReconcileError};
use crate::controller::ControllerContext;
use crate::crd::MCPGatewayConfig;
use crate::nats::ConfigKvError;
use crate::projection::{
    build_status, encode_gateway_config, project_mcp_gateway_config, ProjectionError,
};

pub async fn run_mcp_gateway_config_controller(
    ctx: Arc<ControllerContext>,
    watch_namespace: Option<String>,
) -> Result<(), ReconcileError> {
    let api = match &watch_namespace {
        Some(namespace) => Api::<MCPGatewayConfig>::namespaced(ctx.client.clone(), namespace),
        None => Api::<MCPGatewayConfig>::all(ctx.client.clone()),
    };
    run_controller(
        api,
        watcher_config(),
        ctx,
        reconcile_mcp_gateway_config,
    )
    .await
}

async fn reconcile_mcp_gateway_config(
    resource: Arc<MCPGatewayConfig>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ConfigKvError> {
    if resource.meta().deletion_timestamp.is_some() {
        let projection = project_mcp_gateway_config(&resource)
            .map_err(map_projection_error)?;
        ctx.kv.delete_key(&projection.key).await?;
        return Ok(Action::await_change());
    }

    let projection = project_mcp_gateway_config(&resource).map_err(map_projection_error)?;
    let config_bytes = encode_gateway_config(&projection.config).map_err(map_projection_error)?;
    let revision = publish_bytes(
        &ctx.kv,
        &projection.key,
        &config_bytes,
        &projection.content_hash,
    )
    .await?;

    if let Some(pointer) = &projection.active_pointer {
        let pointer_bytes = serde_json::to_vec(pointer).map_err(|error| {
            ConfigKvError::Put(format!("active pointer json: {error}"))
        })?;
        let pointer_hash = crate::projection::content_hash_hex(&pointer_bytes);
        publish_bytes(&ctx.kv, "bundle/active", &pointer_bytes, &pointer_hash).await?;
        let _ = revision;
    }

    let status = build_status(&resource, &projection, revision);
    let api: Api<MCPGatewayConfig> = Api::namespaced(
        ctx.client.clone(),
        &resource.namespace().expect("namespaced resource"),
    );
    let patch = serde_json::json!({ "status": status });
    api.patch_status(
        &resource.name_any(),
        &PatchParams::default(),
        &Patch::Merge(patch),
    )
    .await
    .map_err(|error| ConfigKvError::Put(error.to_string()))?;

    Ok(Action::await_change())
}

fn map_projection_error(error: ProjectionError) -> ConfigKvError {
    ConfigKvError::Put(error.to_string())
}
