use serde::Serialize;
use trogon_sandbox_contract::NativeSnapshotId;

use crate::{CpuCores, ModalAppName};

/// What the worker asks Modal to create. Field names are Trogon's; the transport maps them onto the
/// `modal.Sandbox.create` keyword arguments.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModalSandboxSpec {
    pub app_name: ModalAppName,
    pub image: String,
    /// Id of a `modal.Image` from a filesystem snapshot. When set the sandbox is built from it and `image` is
    /// not pulled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<NativeSnapshotId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    pub workdir: String,
    pub timeout_seconds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<CpuCores>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mib: Option<u32>,
    /// Region Modal should place the sandbox in; unset lets Modal choose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    pub env: Vec<(String, String)>,
    pub tags: Vec<(String, String)>,
    pub block_network: bool,
    pub outbound_cidr_allowlist: Vec<String>,
    /// Container ports Modal publishes through a TLS tunnel. Modal opens tunnels only at create time.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub encrypted_ports: Vec<u16>,
}
