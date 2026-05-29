use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::controller::Error as ControllerError;
use kube::runtime::watcher::Config;
use kube::Api;
use serde::de::DeserializeOwned;

use crate::controller::ControllerContext;
use crate::nats::{ConfigKv, ConfigKvError, ConfigKvPutOutcome, PutOptions};

pub type ReconcileError = ControllerError<ConfigKvError, std::convert::Infallible>;

pub fn watcher_config() -> Config {
    Config::default()
}

pub async fn publish_bytes(
    kv: &Arc<dyn ConfigKv>,
    key: &str,
    value: &[u8],
    content_hash: &str,
) -> Result<u64, ConfigKvError> {
    match kv
        .put_idempotent(
            key,
            value,
            PutOptions {
                content_hash: content_hash.to_string(),
            },
        )
        .await?
    {
        ConfigKvPutOutcome::Unchanged => Ok(0),
        ConfigKvPutOutcome::Written { revision } => Ok(revision),
    }
}

pub async fn run_controller<K, ReconcileFn, Fut>(
    api: Api<K>,
    watcher_config: Config,
    ctx: Arc<ControllerContext>,
    reconcile_fn: ReconcileFn,
) -> Result<(), ReconcileError>
where
    K: kube::Resource<DynamicType = ()>
        + Clone
        + std::fmt::Debug
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    ReconcileFn: Fn(Arc<K>, Arc<ControllerContext>) -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = Result<Action, ConfigKvError>> + Send + 'static,
{
    Controller::new(api, watcher_config)
        .shutdown_on_signal()
        .run(
            move |object, context| {
                let reconcile_fn = reconcile_fn.clone();
                async move { reconcile_fn(object, context).await }
            },
            |_: Arc<K>, err: &ConfigKvError, _| {
                tracing::warn!(error = %err, "reconcile failed; requeueing");
                Action::requeue(Duration::from_secs(5))
            },
            ctx,
        )
        .for_each(|result| async move {
            if let Err(error) = result {
                tracing::error!(%error, "controller terminated");
            }
        })
        .await;
    Ok(())
}
