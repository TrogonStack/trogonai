use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use base64::Engine as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt as _;
use trogon_sandbox_contract::{NativeSnapshotId, SandboxId};

use crate::bridge_requests::{
    BridgeFailure, BridgeResponse, ExecRequest, SandboxRequest, SnapshotRequest, SnapshotResponse, TunnelsResponse,
    WriteFileRequest,
};
use crate::constants::{BRIDGE_SCRIPT_NAME, BRIDGE_SOURCE, SNAPSHOT_TIMEOUT_SECONDS};
use crate::{ExecResult, ModalApiError, ModalSandbox, ModalSandboxApi, ModalSandboxSpec, ModalTunnel};

/// Drives the official Modal Python SDK through the embedded one-shot JSON bridge.
///
/// The bridge script is compiled into the binary and unpacked into a private temp directory at construction, so
/// the deployed image needs a Python interpreter with `modal` installed but no checked-out copy of this
/// repository. The Modal SDK reads its credentials from the child's inherited environment; this worker never
/// reads, logs, or forwards them itself.
pub struct PythonBridgeModalClient {
    python: String,
    script: PathBuf,
    _scratch: TempDir,
}

impl PythonBridgeModalClient {
    pub fn new(python: impl Into<String>) -> Result<Self, ModalApiError> {
        let scratch = TempDir::new()?;
        let script = scratch.path().join(BRIDGE_SCRIPT_NAME);
        std::fs::write(&script, BRIDGE_SOURCE)?;
        Ok(Self {
            python: python.into(),
            script,
            _scratch: scratch,
        })
    }

    async fn call<Q: Serialize, R: DeserializeOwned>(&self, op: &str, request: &Q) -> Result<R, ModalApiError> {
        let mut child = tokio::process::Command::new(&self.python)
            .arg(&self.script)
            .arg(op)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| ModalApiError::Spawn {
                python: self.python.clone(),
                source,
            })?;
        let payload = serde_json::to_vec(request)?;
        let mut stdin = child.stdin.take().ok_or(ModalApiError::StdinUnavailable)?;
        // A bridge that exits before reading closes the pipe, and its exit status says why.
        let _ = stdin.write_all(&payload).await;
        drop(stdin);
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Err(ModalApiError::Exited {
                op: op.to_owned(),
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(serde_json::from_value(parse_bridge_response(&output.stdout)?)?)
    }
}

fn parse_bridge_response(stdout: &[u8]) -> Result<serde_json::Value, ModalApiError> {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or(ModalApiError::NoOutput)?;
    let response: BridgeResponse = serde_json::from_str(line)?;
    if response.ok {
        return Ok(response.result);
    }
    let failure = response.error.unwrap_or_else(unknown_failure);
    Err(match failure.kind.as_str() {
        "AuthError" | "InvalidError" if failure.message.to_lowercase().contains("token") => {
            ModalApiError::Auth(failure.message)
        }
        "NotFoundError" => ModalApiError::SandboxNotFound(failure.message),
        _ => ModalApiError::Modal {
            kind: failure.kind,
            message: failure.message,
        },
    })
}

fn unknown_failure() -> BridgeFailure {
    BridgeFailure {
        kind: "unknown".to_owned(),
        message: "bridge reported failure without detail".to_owned(),
    }
}

#[async_trait]
impl ModalSandboxApi for PythonBridgeModalClient {
    async fn create_sandbox(&self, spec: &ModalSandboxSpec) -> Result<ModalSandbox, ModalApiError> {
        self.call("create", spec).await
    }

    async fn get_sandbox(&self, sandbox_id: &SandboxId) -> Result<ModalSandbox, ModalApiError> {
        self.call("get", &SandboxRequest { sandbox_id }).await
    }

    async fn exec(&self, sandbox_id: &SandboxId, command: &[String]) -> Result<ExecResult, ModalApiError> {
        self.call("exec", &ExecRequest { sandbox_id, command }).await
    }

    async fn write_file(&self, sandbox_id: &SandboxId, remote_path: &str, data: &[u8]) -> Result<(), ModalApiError> {
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(data);
        let request = WriteFileRequest {
            sandbox_id,
            remote_path,
            data_b64,
        };
        self.call::<_, serde_json::Value>("write_file", &request).await?;
        Ok(())
    }

    async fn terminate(&self, sandbox_id: &SandboxId) -> Result<(), ModalApiError> {
        self.call::<_, serde_json::Value>("terminate", &SandboxRequest { sandbox_id })
            .await?;
        Ok(())
    }

    async fn tunnels(&self, sandbox_id: &SandboxId) -> Result<Vec<ModalTunnel>, ModalApiError> {
        let response: TunnelsResponse = self.call("tunnels", &SandboxRequest { sandbox_id }).await?;
        Ok(response.tunnels)
    }

    async fn snapshot_filesystem(
        &self,
        sandbox_id: &SandboxId,
        ttl_seconds: u64,
    ) -> Result<NativeSnapshotId, ModalApiError> {
        let request = SnapshotRequest {
            sandbox_id,
            timeout_seconds: SNAPSHOT_TIMEOUT_SECONDS,
            ttl_seconds,
        };
        let response: SnapshotResponse = self.call("snapshot", &request).await?;
        Ok(response.image_id)
    }
}
