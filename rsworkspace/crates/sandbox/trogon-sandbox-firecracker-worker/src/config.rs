//! Operator configuration.

use std::time::Duration;

use trogon_sandbox_contract::OciImageRef;

pub use crate::settings::{
    BaseRootfs, ConfigError, DEFAULT_AGENT_VSOCK_PORT, EgressProxyBinary, FirecrackerBinary, GuestCid, KernelImage,
    MemoryMib, NetnsHelper, NetnsPrefix, NetworkConfig, OciRootfsConfig, RelayBinary, RetentionConfig, SnapshotConfig,
    SnapshotDir, StateDir, VcpuCount, VsockPort, WarmPoolConfig,
};

mod constants;
pub use constants::DEFAULT_BOOT_ARGS;

#[derive(Debug, Clone)]
pub struct FirecrackerConfig {
    pub binary: FirecrackerBinary,
    pub kernel: KernelImage,
    pub base_rootfs: BaseRootfs,
    pub state_dir: StateDir,
    pub default_vcpus: VcpuCount,
    /// Used when a lease asks for no memory of its own (`memory_mib == 0`).
    pub default_memory: MemoryMib,
    pub agent_port: VsockPort,
    pub guest_cid: GuestCid,
    /// Budget for each of: the VMM API socket appearing, and the guest agent answering.
    pub boot_timeout: Duration,
    pub boot_args: String,
    /// The only image a lease may name besides leaving it unspecified. Empty means the base
    /// rootfs is reachable only by leaving the image unspecified.
    pub default_image: OciImageRef,
    pub max_concurrent_leases: u32,
    pub warm_pool: WarmPoolConfig,
    pub oci_rootfs: Option<OciRootfsConfig>,
    pub released_snapshots: RetentionConfig,
    pub snapshot_rootfs_max_bytes: u64,
    pub network: Option<crate::host_links::NetworkConfig>,
    pub legacy_network: Option<NetworkConfig>,
    /// NATS URL of the credential broker. With host-links networking it lets a lease's egress
    /// proxy resolve `Transform` rules, injecting credentials into requests the guest sends
    /// through the proxy's interception CA. Unset, such policies are refused.
    pub credential_broker_nats_url: Option<String>,
    /// PKCS#8 PEM EC key a lease's egress proxy signs `Forward` rules' OIDC tokens with. The
    /// key stays on the host; a receiver trusts its public half. Unset, `Forward` rules are
    /// refused.
    pub oidc_signing_key_pem_file: Option<std::path::PathBuf>,
    pub snapshots: Option<SnapshotConfig>,
    /// Static CPU template passed to the VMM (`C3`, `T2`, ...). Part of the environment
    /// digest because it changes the CPU features a snapshot is taken with.
    pub cpu_template: Option<String>,
    /// This worker's environment digest, the digests its operator approved and the policy
    /// for snapshots that record none. The default computes no digest and checks nothing.
    pub environment: trogon_sandbox_protocol::EnvironmentPolicy,
    /// Cosign verification applied to snapshots pulled from the registry on restore.
    pub restore_signature: crate::restore_signature::RestoreSignature,
}

impl FirecrackerConfig {
    pub fn has_network(&self) -> bool {
        self.network.is_some() || self.legacy_network.is_some()
    }

    /// Whether leases can have the proxy terminate TLS. Host-links mode installs the proxy's
    /// CA in the guest before the lease is ready. The legacy helper mode does the same only
    /// with a credential broker, which moves its leases onto an in-process proxy; without
    /// one it keeps the separate `sandbox-egress-proxyd`, which never terminates TLS.
    pub fn intercepts_tls(&self) -> bool {
        self.network.is_some() || (self.legacy_network.is_some() && self.credential_broker_nats_url.is_some())
    }

    /// Whether leases can have credentials injected into the requests they send.
    pub fn injects_credentials(&self) -> bool {
        self.intercepts_tls() && self.credential_broker_nats_url.is_some()
    }

    /// Whether leases can forward requests to another service under their OIDC identity.
    pub fn forwards_requests(&self) -> bool {
        self.network.is_some() && self.oidc_signing_key_pem_file.is_some()
    }

    pub fn new(binary: FirecrackerBinary, kernel: KernelImage, base_rootfs: BaseRootfs, state_dir: StateDir) -> Self {
        Self {
            binary,
            kernel,
            base_rootfs,
            state_dir,
            default_vcpus: VcpuCount::DEFAULT,
            default_memory: MemoryMib::DEFAULT,
            agent_port: VsockPort::DEFAULT_AGENT,
            guest_cid: GuestCid::DEFAULT,
            boot_timeout: Duration::from_secs(30),
            boot_args: DEFAULT_BOOT_ARGS.to_string(),
            default_image: OciImageRef::parse_unqualified(""),
            max_concurrent_leases: 8,
            warm_pool: Default::default(),
            oci_rootfs: None,
            released_snapshots: Default::default(),
            snapshot_rootfs_max_bytes: 4 * 1024 * 1024 * 1024,
            network: None,
            legacy_network: None,
            credential_broker_nats_url: None,
            oidc_signing_key_pem_file: None,
            snapshots: None,
            cpu_template: None,
            environment: Default::default(),
            restore_signature: crate::restore_signature::RestoreSignature::required(),
        }
    }
}

#[cfg(test)]
mod tests;
