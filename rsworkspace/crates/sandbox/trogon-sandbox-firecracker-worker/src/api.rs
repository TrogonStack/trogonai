//! A small client for Firecracker's REST API on its `--api-sock` Unix socket.
//!
//! Firecracker publishes an OpenAPI spec but no Rust client, and the surface needed to boot,
//! snapshot and restore a VM is a handful of calls, so the request bodies are written out here
//! rather than generated.

use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use tokio::net::UnixStream;
use trogon_sandbox_contract::LeaseId;

use crate::config::{FirecrackerConfig, GuestCid, KernelImage, MemoryMib, VcpuCount};
use crate::guest_link;
use crate::vm_dir::VmDir;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("firecracker API at {socket}: {message}")]
    Transport { socket: PathBuf, message: String },
    #[error("firecracker rejected {method} {path} with HTTP {status}: {body}")]
    Rejected {
        method: &'static str,
        path: &'static str,
        status: u16,
        body: String,
    },
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct BootSource {
    pub kernel_image_path: PathBuf,
    pub boot_args: String,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct Drive {
    pub drive_id: &'static str,
    pub path_on_host: PathBuf,
    pub is_root_device: bool,
    pub is_read_only: bool,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct MachineConfig {
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_template: Option<String>,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct VsockDevice {
    pub guest_cid: u32,
    pub uds_path: PathBuf,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct NetworkInterface {
    pub iface_id: &'static str,
    pub host_dev_name: &'static str,
    pub guest_mac: &'static str,
}

impl NetworkInterface {
    /// The lease namespace's tap, whose fixed name and addresses come from `guest_link`.
    pub const LEASE_TAP: Self = Self {
        iface_id: guest_link::GUEST_INTERFACE,
        host_dev_name: guest_link::TAP_DEVICE,
        guest_mac: guest_link::GUEST_MAC,
    };
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct Action {
    pub action_type: &'static str,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct VmState {
    pub state: &'static str,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct SnapshotCreate {
    pub snapshot_type: &'static str,
    pub snapshot_path: PathBuf,
    pub mem_file_path: PathBuf,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct MemoryBackend {
    pub backend_type: &'static str,
    pub backend_path: PathBuf,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct SnapshotLoad {
    pub snapshot_path: PathBuf,
    pub mem_backend: MemoryBackend,
    pub resume_vm: bool,
    pub clock_realtime: bool,
}

impl BootSource {
    pub fn new(kernel: &KernelImage, boot_args: &str) -> Self {
        Self {
            kernel_image_path: kernel.as_path().to_path_buf(),
            boot_args: boot_args.to_string(),
        }
    }
}

impl Drive {
    pub fn root(rootfs: PathBuf) -> Self {
        Self {
            drive_id: "rootfs",
            path_on_host: rootfs,
            is_root_device: true,
            is_read_only: false,
        }
    }
}

impl MachineConfig {
    pub fn new(vcpus: VcpuCount, memory: MemoryMib) -> Self {
        Self {
            vcpu_count: vcpus.get(),
            mem_size_mib: memory.get(),
            cpu_template: None,
        }
    }
}

impl VsockDevice {
    pub fn new(cid: GuestCid, uds_path: PathBuf) -> Self {
        Self {
            guest_cid: cid.get(),
            uds_path,
        }
    }
}

impl Action {
    pub const INSTANCE_START: Self = Self {
        action_type: "InstanceStart",
    };
}

impl VmState {
    pub const PAUSED: Self = Self { state: "Paused" };
    pub const RESUMED: Self = Self { state: "Resumed" };
}

impl SnapshotCreate {
    pub fn full(snapshot_path: PathBuf, mem_file_path: PathBuf) -> Self {
        Self {
            snapshot_type: "Full",
            snapshot_path,
            mem_file_path,
        }
    }
}

impl SnapshotLoad {
    /// The memory file is mapped privately, so one file can back any number of restored VMs
    /// as long as nothing writes to it.
    pub fn resumed(snapshot_path: PathBuf, memory: PathBuf) -> Self {
        Self {
            snapshot_path,
            mem_backend: MemoryBackend {
                backend_type: "File",
                backend_path: memory,
            },
            resume_vm: true,
            clock_realtime: true,
        }
    }
}

/// The guest agent takes its identity from `ate.actor_id=` on the kernel command line.
pub fn boot_args_for(base: &str, actor_id: &LeaseId) -> String {
    format!("{base} ate.actor_id={}", actor_id.as_str())
}

#[derive(Debug, Clone)]
pub struct FirecrackerApi {
    socket: PathBuf,
}

impl FirecrackerApi {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    pub async fn set_boot_source(&self, body: &BootSource) -> Result<(), ApiError> {
        self.put("/boot-source", body).await
    }

    pub async fn set_root_drive(&self, body: &Drive) -> Result<(), ApiError> {
        self.put("/drives/rootfs", body).await
    }

    pub async fn set_machine_config(&self, body: &MachineConfig) -> Result<(), ApiError> {
        self.put("/machine-config", body).await
    }

    pub async fn set_vsock(&self, body: &VsockDevice) -> Result<(), ApiError> {
        self.put("/vsock", body).await
    }

    pub async fn set_network_interface(&self, body: &NetworkInterface) -> Result<(), ApiError> {
        self.put("/network-interfaces/eth0", body).await
    }

    pub async fn start_instance(&self) -> Result<(), ApiError> {
        self.put("/actions", &Action::INSTANCE_START).await
    }

    pub async fn pause(&self) -> Result<(), ApiError> {
        self.send(Method::PATCH, "/vm", &VmState::PAUSED, CALL_TIMEOUT).await
    }

    pub async fn resume(&self) -> Result<(), ApiError> {
        self.send(Method::PATCH, "/vm", &VmState::RESUMED, CALL_TIMEOUT).await
    }

    /// Only accepted while the VM is paused.
    pub async fn create_snapshot(&self, body: &SnapshotCreate) -> Result<(), ApiError> {
        self.send(Method::PUT, "/snapshot/create", body, SNAPSHOT_TIMEOUT).await
    }

    /// Only accepted by a fresh process that has not been configured. The snapshot reopens
    /// the drive and vsock socket at the paths it was taken with, which are relative to the
    /// VMM's working directory, so the restored VM uses the files of the directory it is
    /// launched in.
    pub async fn load_snapshot(&self, body: &SnapshotLoad) -> Result<(), ApiError> {
        self.send(Method::PUT, "/snapshot/load", body, SNAPSHOT_TIMEOUT).await
    }

    pub async fn create_snapshot_files(&self, state: PathBuf, memory: PathBuf) -> Result<(), ApiError> {
        self.create_snapshot(&SnapshotCreate::full(state, memory)).await
    }

    pub async fn load_snapshot_files(&self, state: PathBuf, memory: PathBuf) -> Result<(), ApiError> {
        self.load_snapshot(&SnapshotLoad::resumed(state, memory)).await
    }

    /// Runs the full pre-boot sequence, in the order Firecracker requires: everything is
    /// configured before `InstanceStart`, and nothing can be changed after it. Without
    /// `networked` the guest gets no network device at all.
    pub async fn configure_and_start(
        &self,
        config: &FirecrackerConfig,
        memory: MemoryMib,
        actor_id: &LeaseId,
        networked: bool,
    ) -> Result<(), ApiError> {
        let base = if networked {
            format!("{} {}", config.boot_args, guest_link::guest_ip_boot_arg())
        } else {
            config.boot_args.clone()
        };
        let boot_args = boot_args_for(&base, actor_id);
        self.set_boot_source(&BootSource::new(&config.kernel, &boot_args))
            .await?;
        self.set_root_drive(&Drive::root(VmDir::rootfs_from_vmm())).await?;
        let mut machine = MachineConfig::new(config.default_vcpus, memory);
        machine.cpu_template = config.cpu_template.clone();
        self.set_machine_config(&machine).await?;
        self.set_vsock(&VsockDevice::new(config.guest_cid, VmDir::vsock_socket_from_vmm()))
            .await?;
        if networked {
            self.set_network_interface(&NetworkInterface::LEASE_TAP).await?;
        }
        self.start_instance().await
    }

    async fn put<T: Serialize>(&self, path: &'static str, body: &T) -> Result<(), ApiError> {
        self.send(Method::PUT, path, body, CALL_TIMEOUT).await
    }

    async fn send<T: Serialize>(
        &self,
        method: Method,
        path: &'static str,
        body: &T,
        timeout: Duration,
    ) -> Result<(), ApiError> {
        let name = method_name(&method);
        match tokio::time::timeout(timeout, self.send_once(method, path, body)).await {
            Ok(result) => result,
            Err(_) => Err(self.transport(format!("{name} {path} timed out"))),
        }
    }

    async fn send_once<T: Serialize>(&self, method: Method, path: &'static str, body: &T) -> Result<(), ApiError> {
        let name = method_name(&method);
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|e| self.transport(format!("connect: {e}")))?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| self.transport(format!("handshake: {e}")))?;
        tokio::spawn(connection);

        let payload = serde_json::to_vec(body).map_err(|e| self.transport(format!("encoding {name} {path}: {e}")))?;
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::HOST, "localhost")
            .header(hyper::header::ACCEPT, "application/json")
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(payload)))
            .map_err(|e| self.transport(format!("building {name} {path}: {e}")))?;

        let response = sender
            .send_request(request)
            .await
            .map_err(|e| self.transport(format!("{name} {path}: {e}")))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response
            .into_body()
            .collect()
            .await
            .map(|collected| collected.to_bytes())
            .unwrap_or_default();
        Err(ApiError::Rejected {
            method: name,
            path,
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).chars().take(512).collect(),
        })
    }

    fn transport(&self, message: String) -> ApiError {
        ApiError::Transport {
            socket: self.socket.clone(),
            message,
        }
    }
}

fn method_name(method: &Method) -> &'static str {
    if method == Method::PATCH { "PATCH" } else { "PUT" }
}

#[cfg(test)]
mod tests;

mod constants;
use constants::{CALL_TIMEOUT, SNAPSHOT_TIMEOUT};
