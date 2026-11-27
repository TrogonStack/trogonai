use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::{DeleteParams, ListParams, LogParams, PostParams};
use kube::core::DynamicObject;
use serde_json::Value;
use tokio::time::{Instant, sleep};

use super::inspect::{has_condition, pvc_from_snapshot_content};
use super::manifest::{export_pod, object};
use super::resources::{snapshot_content_resource, snapshot_resource};
use super::{KubeVirtOps, found, require};
use crate::constants::{EXPORT_POD_TIMEOUT, KUBEVIRT_POLL_INTERVAL, KUBEVIRT_WAIT_TIMEOUT, SNAPSHOT_SOURCE_LABEL};
use crate::{KubeVirtOpsError, resource_name};

impl KubeVirtOps {
    /// Asks KubeVirt to snapshot `vm_name` as `snapshot_name`.
    pub async fn create_vm_snapshot(
        &self,
        snapshot_name: &str,
        vm_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<(), KubeVirtOpsError> {
        // KubeVirt selects online vs offline snapshot semantics from VM power state. When the VM is running this
        // creates an online snapshot (guest-agent freeze when present, crash-consistent otherwise) without
        // stopping the VM.
        let source = object([
            ("apiGroup", "kubevirt.io".into()),
            ("kind", "VirtualMachine".into()),
            ("name", vm_name.into()),
        ]);
        let mut snapshot = DynamicObject::new(snapshot_name, &snapshot_resource())
            .within(&self.config.namespace)
            .data(object([("spec", object([("source", source)]))]));
        snapshot.metadata.labels = Some(labels);
        self.api(&snapshot_resource())
            .create(&PostParams::default(), &snapshot)
            .await?;
        Ok(())
    }

    /// Deletes the snapshot `name`. A missing snapshot is not an error.
    pub async fn delete_vm_snapshot(&self, name: &str) -> Result<(), KubeVirtOpsError> {
        found(
            self.api(&snapshot_resource())
                .delete(name, &DeleteParams::default())
                .await,
        )?;
        Ok(())
    }

    /// Waits up to [`crate::KUBEVIRT_WAIT_TIMEOUT`] for the snapshot `name` to be ready to use, failing early on
    /// a `Failure` condition.
    pub async fn wait_vm_snapshot_ready(&self, name: &str) -> Result<(), KubeVirtOpsError> {
        let api = self.api(&snapshot_resource());
        let deadline = Instant::now() + KUBEVIRT_WAIT_TIMEOUT;
        loop {
            let snapshot = api.get(name).await?;
            let ready = snapshot
                .data
                .get("status")
                .and_then(|status| status.get("readyToUse"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if ready {
                break Ok(());
            }
            require(
                !has_condition(&snapshot.data, "Failure"),
                KubeVirtOpsError::SnapshotFailed { name: name.to_owned() },
            )?;
            if Instant::now() >= deadline {
                break Err(KubeVirtOpsError::SnapshotTimeout { name: name.to_owned() });
            }
            sleep(KUBEVIRT_POLL_INTERVAL).await;
        }
    }

    /// The PVC the snapshot's content backs its first volume up to. Contents labelled with the snapshot are
    /// tried first, then any content whose name contains the snapshot name.
    pub async fn snapshot_pvc_name(&self, snapshot_name: &str) -> Result<String, KubeVirtOpsError> {
        let api = self.api(&snapshot_content_resource());
        let labelled = ListParams::default().labels(&format!("{SNAPSHOT_SOURCE_LABEL}={snapshot_name}"));
        let labelled = api
            .list(&labelled)
            .await
            .ok()
            .and_then(|listing| listing.items.iter().find_map(pvc_from_snapshot_content));
        if let Some(pvc) = labelled {
            return Ok(pvc);
        }
        let listing = api.list(&ListParams::default()).await?;
        listing
            .items
            .iter()
            .filter(|item| {
                item.metadata
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .contains(snapshot_name)
            })
            .find_map(pvc_from_snapshot_content)
            .ok_or_else(|| KubeVirtOpsError::SnapshotPvcMissing {
                name: snapshot_name.to_owned(),
            })
    }

    /// Every byte of the files on `pvc_name`, streamed through the log of a short-lived read-only export pod
    /// that may run up to [`crate::EXPORT_POD_TIMEOUT`].
    pub async fn read_pvc_bytes(&self, pvc_name: &str) -> Result<Vec<u8>, KubeVirtOpsError> {
        let pod_name = resource_name("export", pvc_name);
        let _stale = self.delete_export_pod(&pod_name).await;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let pod = export_pod(&pod_name, &self.config.namespace, pvc_name);
        pods.create(&PostParams::default(), &pod).await?;
        let deadline = Instant::now() + EXPORT_POD_TIMEOUT;
        loop {
            let phase = pods.get(&pod_name).await?.status.and_then(|status| status.phase);
            if matches!(phase.as_deref(), Some("Succeeded" | "Failed")) {
                break;
            }
            if Instant::now() >= deadline {
                let _abandoned = self.delete_export_pod(&pod_name).await;
                return Err(KubeVirtOpsError::ExportPodTimeout { name: pod_name });
            }
            sleep(KUBEVIRT_POLL_INTERVAL).await;
        }
        let logs = pods.logs(&pod_name, &LogParams::default()).await?;
        self.delete_export_pod(&pod_name).await?;
        require(
            !logs.is_empty(),
            KubeVirtOpsError::EmptyExport {
                pvc: pvc_name.to_owned(),
            },
        )?;
        Ok(logs.into_bytes())
    }

    async fn delete_export_pod(&self, name: &str) -> Result<(), KubeVirtOpsError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.namespace);
        found(pods.delete(name, &DeleteParams::default()).await)?;
        Ok(())
    }
}
