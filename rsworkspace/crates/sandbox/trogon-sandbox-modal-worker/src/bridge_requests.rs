use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{NativeSnapshotId, SandboxId};

use crate::ModalTunnel;

#[derive(Serialize)]
pub(crate) struct SandboxRequest<'a> {
    pub(crate) sandbox_id: &'a SandboxId,
}

#[derive(Serialize)]
pub(crate) struct ExecRequest<'a> {
    pub(crate) sandbox_id: &'a SandboxId,
    pub(crate) command: &'a [String],
}

#[derive(Serialize)]
pub(crate) struct WriteFileRequest<'a> {
    pub(crate) sandbox_id: &'a SandboxId,
    pub(crate) remote_path: &'a str,
    pub(crate) data_b64: String,
}

#[derive(Serialize)]
pub(crate) struct SnapshotRequest<'a> {
    pub(crate) sandbox_id: &'a SandboxId,
    pub(crate) timeout_seconds: u64,
    pub(crate) ttl_seconds: u64,
}

#[derive(Deserialize)]
pub(crate) struct TunnelsResponse {
    pub(crate) tunnels: Vec<ModalTunnel>,
}

#[derive(Deserialize)]
pub(crate) struct SnapshotResponse {
    pub(crate) image_id: NativeSnapshotId,
}

#[derive(Deserialize)]
pub(crate) struct BridgeFailure {
    pub(crate) kind: String,
    pub(crate) message: String,
}

#[derive(Deserialize)]
pub(crate) struct BridgeResponse {
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) result: serde_json::Value,
    #[serde(default)]
    pub(crate) error: Option<BridgeFailure>,
}
