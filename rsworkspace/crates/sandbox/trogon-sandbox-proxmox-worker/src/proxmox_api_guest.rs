use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::Value;
use tokio::time::Instant;
use trogon_sandbox_protocol::FileNotFoundError;

use crate::api_bodies::ExecBody;
use crate::api_wire::{Envelope, decode_response, is_missing_file_error};
use crate::constants::{AGENT_POLL, EXEC_POLL, FILE_READ_COUNT};
use crate::guest_process::GuestProcess;
use crate::{ProxmoxApi, ProxmoxApiError, VmId};

impl ProxmoxApi {
    /// Waits until the QEMU guest agent answers a ping, within `budget`.
    pub async fn wait_agent(&self, vm: VmId, budget: Duration) -> Result<(), ProxmoxApiError> {
        let vm = vm.to_string();
        tokio::time::timeout(budget, async {
            loop {
                let ping: Result<Value, _> = self.request(Method::POST, &self.vm_path(&vm, &["agent", "ping"])).await;
                if ping.is_ok() {
                    return;
                }
                tokio::time::sleep(AGENT_POLL).await;
            }
        })
        .await
        .map_err(|_| ProxmoxApiError::AgentTimedOut)
    }

    /// One chunk of a guest file starting at `offset`, and whether more follows.
    pub(crate) async fn read_file_chunk(
        &self,
        vm: VmId,
        file: &str,
        offset: usize,
    ) -> Result<(String, bool), ProxmoxApiError> {
        let vm = vm.to_string();
        let mut url = self.url(&self.vm_path(&vm, &["agent", "file-read"]))?;
        url.query_pairs_mut()
            .append_pair("file", file)
            .append_pair("offset", &offset.to_string())
            .append_pair("count", FILE_READ_COUNT)
            .append_pair("decode", "0");
        let response = self.builder(Method::GET, url).send().await?;
        let status = response.status();
        if !status.is_success() {
            // The guest agent reports a missing file as a failed call whose message carries the errno text.
            let detail = response.text().await.unwrap_or_default();
            if is_missing_file_error(&detail) {
                return Err(ProxmoxApiError::FileNotFound(FileNotFoundError { path: file.into() }));
            }
            return Err(ProxmoxApiError::Status(status));
        }
        let body = decode_response::<Envelope<Value>>(response).await?.data;
        let content = body["content"].as_str().ok_or(ProxmoxApiError::NoContent)?.to_owned();
        let truncated = match &body["truncated"] {
            Value::Null | Value::Bool(false) => false,
            Value::Bool(true) => true,
            Value::Number(number) if number.as_u64() == Some(0) => false,
            Value::Number(number) if number.as_u64() == Some(1) => true,
            _ => return Err(ProxmoxApiError::TruncatedFlag),
        };
        Ok((content, truncated))
    }

    /// Runs a command through the guest agent and waits for it, within `budget`.
    pub(crate) async fn exec(
        &self,
        vm: VmId,
        command: &[String],
        budget: Duration,
    ) -> Result<GuestProcess, ProxmoxApiError> {
        let vm = vm.to_string();
        let start = Instant::now();
        tokio::time::timeout(budget, async {
            let started: Value = self
                .request_with(
                    Method::POST,
                    &self.vm_path(&vm, &["agent", "exec"]),
                    &ExecBody { command },
                )
                .await?;
            let pid = started["pid"].as_u64().ok_or(ProxmoxApiError::NoPid)?;
            loop {
                let mut url = self.url(&self.vm_path(&vm, &["agent", "exec-status"]))?;
                url.query_pairs_mut().append_pair("pid", &pid.to_string());
                let response = self.builder(Method::GET, url).send().await?;
                if response.status() != StatusCode::OK {
                    return Err(ProxmoxApiError::ExecStatus);
                }
                let result = response.json::<Envelope<GuestProcess>>().await?.data;
                if result.exited == Some(1) {
                    if result.stdout_truncated.unwrap_or(0) != 0 || result.stderr_truncated.unwrap_or(0) != 0 {
                        return Err(ProxmoxApiError::ExecTruncated);
                    }
                    return Ok(result);
                }
                if start.elapsed() >= budget {
                    return Err(ProxmoxApiError::ExecTimedOut);
                }
                tokio::time::sleep(EXEC_POLL).await;
            }
        })
        .await
        .map_err(|_| ProxmoxApiError::ExecTimedOut)?
    }
}
