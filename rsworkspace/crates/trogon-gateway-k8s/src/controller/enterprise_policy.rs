use std::sync::Arc;

use kube::Resource;
use kube::ResourceExt;
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::Action;

use crate::controller::ControllerContext;
use crate::controller::policy::{ReconcileError, publish_bytes, run_controller, watcher_config};
use crate::crd::EnterpriseAgentgatewayPolicy;
use crate::nats::ConfigKvError;
use crate::projection::{ProjectionError, build_enterprise_policy_status, project_enterprise_policy};

pub async fn run_enterprise_policy_controller(
    ctx: Arc<ControllerContext>,
    watch_namespace: Option<String>,
) -> Result<(), ReconcileError> {
    let api = match &watch_namespace {
        Some(namespace) => Api::<EnterpriseAgentgatewayPolicy>::namespaced(ctx.client.clone(), namespace),
        None => Api::<EnterpriseAgentgatewayPolicy>::all(ctx.client.clone()),
    };
    run_controller(api, watcher_config(), ctx, reconcile).await
}

async fn reconcile(
    resource: Arc<EnterpriseAgentgatewayPolicy>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ConfigKvError> {
    if resource.meta().deletion_timestamp.is_some() {
        let projection = project_enterprise_policy(&resource).map_err(map_projection_error)?;
        ctx.kv.delete_key(&projection.key).await?;
        return Ok(Action::await_change());
    }

    let projection = project_enterprise_policy(&resource).map_err(map_projection_error)?;
    let bytes = serde_json::to_vec(&projection.value).map_err(|e| ConfigKvError::Put(format!("policy json: {e}")))?;
    let revision = publish_bytes(&ctx.kv, &projection.key, &bytes, &projection.content_hash).await?;

    let status = build_enterprise_policy_status(&resource, revision);
    let api: Api<EnterpriseAgentgatewayPolicy> =
        Api::namespaced(ctx.client.clone(), &resource.namespace().expect("namespaced resource"));
    let patch = serde_json::json!({ "status": status });
    api.patch_status(&resource.name_any(), &PatchParams::default(), &Patch::Merge(patch))
        .await
        .map_err(|e| ConfigKvError::Put(e.to_string()))?;

    Ok(Action::await_change())
}

fn map_projection_error(error: ProjectionError) -> ConfigKvError {
    ConfigKvError::Put(error.to_string())
}
