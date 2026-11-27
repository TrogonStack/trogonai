use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::watch;

use crate::k8s_ops::K8sOps;
use crate::{K8sError, K8sOpsError, SandboxError, SandboxHandle, SandboxStatus};

pub(crate) struct K8sMicroVmHandle {
    pub(crate) name: String,
    pub(crate) namespace: String,
    pub(crate) ops: Arc<dyn K8sOps>,
    pub(crate) cancelled: Arc<AtomicBool>,
    pub(crate) clone_secret_name: Option<String>,
    pub(crate) extra_secret_names: Vec<String>,
    pub(crate) network_policy_name: Option<String>,
    pub(crate) config_map_name: Option<String>,
    pub(crate) status_rx: watch::Receiver<SandboxStatus>,
    pub(crate) _watcher_handle: Arc<tokio::task::JoinHandle<()>>,
}

impl K8sMicroVmHandle {
    async fn delete_resources(&self) -> Result<(), K8sOpsError> {
        self.ops.delete_job(&self.namespace, &self.name).await?;
        if let Some(secret_name) = &self.clone_secret_name {
            self.ops.delete_secret(&self.namespace, secret_name).await?;
        }
        for secret_name in &self.extra_secret_names {
            self.ops.delete_secret(&self.namespace, secret_name).await?;
        }
        if let Some(network_policy_name) = &self.network_policy_name {
            self.ops
                .delete_network_policy(&self.namespace, network_policy_name)
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl SandboxHandle for K8sMicroVmHandle {
    fn id(&self) -> &str {
        &self.name
    }

    async fn status(&self) -> SandboxStatus {
        if self.cancelled.load(Ordering::Acquire) {
            return SandboxStatus::Cancelled;
        }
        self.status_rx.borrow().clone()
    }

    async fn cancel(&self) -> Result<(), SandboxError> {
        self.cancelled.store(true, Ordering::Release);
        self.delete_resources().await.map_err(K8sError::from)?;
        Ok(())
    }

    fn network_policy_name(&self) -> Option<&str> {
        self.network_policy_name.as_deref()
    }

    fn config_map_name(&self) -> Option<&str> {
        self.config_map_name.as_deref()
    }
}
