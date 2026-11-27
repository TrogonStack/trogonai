use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde_json::Value;

use crate::api_bodies::{CloneBody, SnapshotBody};
use crate::api_wire::{Envelope, VmIdResponse, decode_response};
use crate::constants::TASK_POLL;
use crate::{ProxmoxApi, ProxmoxApiError, VmId};

impl ProxmoxApi {
    /// The next free VM id.
    pub async fn next_id(&self) -> Result<VmId, ProxmoxApiError> {
        let raw: VmIdResponse = self.request(Method::GET, &["cluster", "nextid"]).await?;
        Ok(raw.validated()?)
    }

    pub async fn config(&self, vm: VmId) -> Result<Value, ProxmoxApiError> {
        let vm = vm.to_string();
        self.request(Method::GET, &self.vm_path(&vm, &["config"])).await
    }

    /// The VM's configuration, or `None` once the whole cluster agrees the id is free.
    pub async fn config_if_present(&self, vm: VmId) -> Result<Option<Value>, ProxmoxApiError> {
        let vm_text = vm.to_string();
        let response = self
            .builder(Method::GET, self.url(&self.vm_path(&vm_text, &["config"]))?)
            .send()
            .await?;
        let status = response.status();
        if !matches!(
            status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::INTERNAL_SERVER_ERROR
        ) {
            if !status.is_success() {
                return Err(ProxmoxApiError::Status(status));
            }
            return Ok(Some(decode_response::<Envelope<Value>>(response).await?.data));
        }
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            let diagnostic: Value = decode_response(response).await?;
            let expected = format!(
                "Configuration file 'nodes/{}/qemu-server/{vm_text}.conf' does not exist",
                self.node
            );
            let confirmed = diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.trim_end() == expected);
            if !confirmed {
                return Err(ProxmoxApiError::LookupUnconfirmed);
            }
        }
        let mut occupancy_url = self.url(&["cluster", "nextid"])?;
        occupancy_url.query_pairs_mut().append_pair("vmid", &vm_text);
        let occupancy = self.builder(Method::GET, occupancy_url).send().await?;
        if !occupancy.status().is_success() {
            return Err(ProxmoxApiError::AbsenceUnverified);
        }
        let free: Envelope<VmIdResponse> = decode_response(occupancy).await?;
        if free.data.validated()? != vm {
            return Err(ProxmoxApiError::AbsenceMismatch);
        }
        Ok(None)
    }

    /// Starts a linked clone of `template` and returns the task that performs it.
    pub async fn begin_linked_clone(&self, template: VmId, vm: VmId, name: &str) -> Result<String, ProxmoxApiError> {
        let template = template.to_string();
        let body = CloneBody {
            newid: u32::from(vm),
            name,
            full: 0,
            pool: self.pool.as_ref(),
        };
        self.request_with(Method::POST, &self.vm_path(&template, &["clone"]), &body)
            .await
    }

    /// Creates a linked clone of `template` and waits until the task that performs it succeeds.
    pub async fn linked_clone(&self, template: VmId, vm: VmId, name: &str) -> Result<(), ProxmoxApiError> {
        let upid = self.begin_linked_clone(template, vm, name).await?;
        self.wait_task(&upid).await
    }

    pub async fn update_config(&self, vm: VmId, config: &impl Serialize) -> Result<(), ProxmoxApiError> {
        let vm = vm.to_string();
        let _: Value = self
            .request_with(Method::PUT, &self.vm_path(&vm, &["config"]), config)
            .await?;
        Ok(())
    }

    pub async fn start(&self, vm: VmId) -> Result<(), ProxmoxApiError> {
        self.vm_task(Method::POST, vm, &["status", "start"]).await
    }

    pub async fn stop(&self, vm: VmId) -> Result<(), ProxmoxApiError> {
        self.vm_task(Method::POST, vm, &["status", "stop"]).await
    }

    /// Starts deleting the VM and returns the task that performs it.
    pub async fn begin_destroy(&self, vm: VmId) -> Result<String, ProxmoxApiError> {
        let vm = vm.to_string();
        self.request(Method::DELETE, &self.vm_path(&vm, &[])).await
    }

    /// Deletes the VM and waits until the task that performs it succeeds.
    pub async fn destroy(&self, vm: VmId) -> Result<(), ProxmoxApiError> {
        let upid = self.begin_destroy(vm).await?;
        self.wait_task(&upid).await
    }

    /// Takes a snapshot named `name` that includes the VM's memory, and waits until it is complete.
    pub async fn snapshot(&self, vm: VmId, name: &str) -> Result<(), ProxmoxApiError> {
        let vm = vm.to_string();
        let body = SnapshotBody {
            snapname: name,
            vmstate: 1,
        };
        let upid: String = self
            .request_with(Method::POST, &self.vm_path(&vm, &["snapshot"]), &body)
            .await?;
        self.wait_task(&upid).await
    }

    /// Returns the VM to the snapshot named `name` and waits until the rollback is complete.
    pub async fn rollback(&self, vm: VmId, name: &str) -> Result<(), ProxmoxApiError> {
        self.vm_task(Method::POST, vm, &["snapshot", name, "rollback"]).await
    }

    async fn vm_task(&self, method: Method, vm: VmId, suffix: &[&str]) -> Result<(), ProxmoxApiError> {
        let vm = vm.to_string();
        let upid: String = self.request(method, &self.vm_path(&vm, suffix)).await?;
        self.wait_task(&upid).await
    }

    pub async fn wait_task(&self, upid: &str) -> Result<(), ProxmoxApiError> {
        if self.wait_task_terminal(upid).await? {
            Ok(())
        } else {
            Err(ProxmoxApiError::TaskFailed)
        }
    }

    /// Waits for a task to stop and reports whether it ended successfully.
    pub async fn wait_task_terminal(&self, upid: &str) -> Result<bool, ProxmoxApiError> {
        tokio::time::timeout(self.task_timeout, async {
            loop {
                let task: Value = self
                    .request(Method::GET, &self.node_path(&["tasks", upid, "status"]))
                    .await?;
                match task["status"].as_str() {
                    Some("stopped") => {
                        let status = task["exitstatus"]
                            .as_str()
                            .ok_or(ProxmoxApiError::TaskWithoutExitStatus)?;
                        return Ok(status == "OK");
                    }
                    Some("running") => tokio::time::sleep(TASK_POLL).await,
                    _ => return Err(ProxmoxApiError::TaskStatus),
                }
            }
        })
        .await
        .map_err(|_| ProxmoxApiError::TaskTimedOut)?
    }
}
