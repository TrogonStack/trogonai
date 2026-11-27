use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::sandbox_spec_wire::SandboxSpecWire;
use crate::{
    BootMode, DriveError, DriveMount, FsPolicy, IdleAction, IdlePolicy, LeaseTtl, NetworkPolicy, PlatformOptions, Port,
    PrivilegedAccess, RemoteMount, RetentionToggles, SandboxImage, SandboxSpecError, SandboxSpecParts, SecretBindings,
    SecretTarget, SnapshotCaptureAccess, SnapshotPolicy, WasmCapabilityGrant, WorkspaceSource,
};

/// Everything a lease asks of its sandbox: image, workspace, environment, resources, network, filesystem, secret
/// bindings, and lifetime.
///
/// A spec accepts only a coherent set of fields, and construction and deserialization both enforce the rules.
/// The image names something to boot. The environment holds at most [`SandboxSpec::MAX_ENV_ENTRIES`] entries, each
/// key non-empty without `=` or NUL and at most [`SandboxSpec::MAX_ENV_KEY_BYTES`] bytes, each value at most
/// [`SandboxSpec::MAX_ENV_VALUE_BYTES`] bytes without NUL, and at most [`SandboxSpec::MAX_ENV_TOTAL_BYTES`] bytes in
/// all. Remote mount paths are distinct, at most [`SandboxSpec::MAX_DRIVES`] drives mount at distinct paths, and at
/// most [`SandboxSpec::MAX_EXPOSED_PORTS`] distinct ports are exposed. The idle timeout and every idle policy stage
/// fall within [`SandboxSpec::MAX_IDLE_SECONDS`], and a spec sets an idle timeout or an idle policy, not both. A
/// lease with no deadline needs an idle timeout or an idle policy release stage. Snapshot output needs snapshot
/// capture allowed.
///
/// Secret bindings are checked against the rest of the spec. Each one needs a user-defined network policy and must
/// not target an environment variable the spec already sets. Every inject host, with its scope's port meaning made
/// explicit, must be covered by an allowed domain and must not be entirely covered by a denied one, because a denied
/// host could never receive the credential. A partial overlap is legitimate: the denied hosts are carved out and
/// never receive it.
///
/// Fields left at their default are omitted from the wire, except the environment, the boot mode, and the privileged
/// flag. The `Debug` output shows environment keys and hides their values.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "SandboxSpecWire", into = "SandboxSpecWire")]
pub struct SandboxSpec {
    image: SandboxImage,
    workspace: WorkspaceSource,
    env: BTreeMap<String, String>,
    cpu_seconds: NonZeroU32,
    memory_mib: NonZeroU32,
    snapshot_out: Option<SnapshotPolicy>,
    network_policy: Option<NetworkPolicy>,
    mounts: Vec<RemoteMount>,
    drives: Vec<DriveMount>,
    exposed_ports: Vec<Port>,
    boot_mode: BootMode,
    working_dir: Option<String>,
    user: Option<String>,
    privileged: PrivilegedAccess,
    wasi_capabilities: Option<WasmCapabilityGrant>,
    fs_policy: Option<FsPolicy>,
    secret_bindings: SecretBindings,
    platform: PlatformOptions,
    idle_timeout_seconds: Option<NonZeroU32>,
    no_ttl: LeaseTtl,
    idle_policy: Option<IdlePolicy>,
    vcpu_millis: Option<u32>,
    disk_mib: Option<u32>,
    retention: RetentionToggles,
}

impl SandboxSpec {
    /// Most drives one sandbox mounts.
    pub const MAX_DRIVES: usize = 4;
    /// Most environment entries one spec carries.
    pub const MAX_ENV_ENTRIES: usize = 128;
    /// Longest environment key, in bytes.
    pub const MAX_ENV_KEY_BYTES: usize = 128;
    /// Longest environment value, in bytes.
    pub const MAX_ENV_VALUE_BYTES: usize = 8 * 1024;
    /// Most bytes all environment keys and values add up to.
    pub const MAX_ENV_TOTAL_BYTES: usize = 64 * 1024;
    /// Most ports one sandbox exposes.
    pub const MAX_EXPOSED_PORTS: usize = 15;
    /// Longest idle time a timeout or a policy stage may name, in seconds.
    pub const MAX_IDLE_SECONDS: u32 = 86_400;

    pub fn new(parts: SandboxSpecParts) -> Result<Self, SandboxSpecError> {
        if parts.image.is_unspecified() {
            return Err(SandboxSpecError::EmptyImage);
        }
        check_env(&parts.env)?;
        check_idle(&parts)?;
        if parts.retention.snapshots == SnapshotCaptureAccess::Refused && parts.snapshot_out.is_some() {
            return Err(SandboxSpecError::SnapshotOutWithoutSnapshots);
        }
        let mut mount_paths = HashSet::new();
        for mount in &parts.mounts {
            if !mount_paths.insert(&mount.mount_path) {
                return Err(SandboxSpecError::DuplicateMountPath(mount.mount_path.clone()));
            }
        }
        check_drives(&parts.drives)?;
        check_exposed_ports(&parts.exposed_ports)?;
        check_secret_bindings(&parts.secret_bindings, &parts.env, parts.network_policy.as_ref())?;
        Ok(Self {
            image: parts.image,
            workspace: parts.workspace,
            env: parts.env,
            cpu_seconds: parts.cpu_seconds,
            memory_mib: parts.memory_mib,
            snapshot_out: parts.snapshot_out,
            network_policy: parts.network_policy,
            mounts: parts.mounts,
            drives: parts.drives,
            exposed_ports: parts.exposed_ports,
            boot_mode: parts.boot_mode,
            working_dir: parts.working_dir,
            user: parts.user,
            privileged: parts.privileged,
            wasi_capabilities: parts.wasi_capabilities,
            fs_policy: parts.fs_policy,
            secret_bindings: parts.secret_bindings,
            platform: parts.platform,
            idle_timeout_seconds: parts.idle_timeout_seconds,
            no_ttl: parts.no_ttl,
            idle_policy: parts.idle_policy,
            vcpu_millis: parts.vcpu_millis,
            disk_mib: parts.disk_mib,
            retention: parts.retention,
        })
    }

    /// Takes the spec apart so a caller can change fields and build it again, which checks the rules again.
    pub fn into_parts(self) -> SandboxSpecParts {
        SandboxSpecParts {
            image: self.image,
            workspace: self.workspace,
            env: self.env,
            cpu_seconds: self.cpu_seconds,
            memory_mib: self.memory_mib,
            snapshot_out: self.snapshot_out,
            network_policy: self.network_policy,
            mounts: self.mounts,
            drives: self.drives,
            exposed_ports: self.exposed_ports,
            boot_mode: self.boot_mode,
            working_dir: self.working_dir,
            user: self.user,
            privileged: self.privileged,
            wasi_capabilities: self.wasi_capabilities,
            fs_policy: self.fs_policy,
            secret_bindings: self.secret_bindings,
            platform: self.platform,
            idle_timeout_seconds: self.idle_timeout_seconds,
            no_ttl: self.no_ttl,
            idle_policy: self.idle_policy,
            vcpu_millis: self.vcpu_millis,
            disk_mib: self.disk_mib,
            retention: self.retention,
        }
    }

    pub fn image(&self) -> &SandboxImage {
        &self.image
    }

    pub fn workspace(&self) -> &WorkspaceSource {
        &self.workspace
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    pub fn cpu_seconds(&self) -> NonZeroU32 {
        self.cpu_seconds
    }

    pub fn memory_mib(&self) -> NonZeroU32 {
        self.memory_mib
    }

    pub fn snapshot_out(&self) -> Option<&SnapshotPolicy> {
        self.snapshot_out.as_ref()
    }

    /// The egress policy, or `None` for the provider default.
    pub fn network_policy(&self) -> Option<&NetworkPolicy> {
        self.network_policy.as_ref()
    }

    pub fn mounts(&self) -> &[RemoteMount] {
        &self.mounts
    }

    pub fn drives(&self) -> &[DriveMount] {
        &self.drives
    }

    /// The ports the sandbox wants published for external reachability. Empty means none.
    pub fn exposed_ports(&self) -> &[Port] {
        &self.exposed_ports
    }

    pub fn boot_mode(&self) -> BootMode {
        self.boot_mode
    }

    /// Overrides the image's working directory for command execution.
    pub fn working_dir(&self) -> Option<&str> {
        self.working_dir.as_deref()
    }

    /// Overrides the container's default user, as `uid`, `uid:gid`, or a user name.
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    pub fn privileged(&self) -> PrivilegedAccess {
        self.privileged
    }

    /// The WASI capability grant, or `None` for a guest that is fully deny-by-default.
    pub fn wasi_capabilities(&self) -> Option<&WasmCapabilityGrant> {
        self.wasi_capabilities.as_ref()
    }

    /// The read and write rules for guest paths, or `None` for the provider default.
    pub fn fs_policy(&self) -> Option<&FsPolicy> {
        self.fs_policy.as_ref()
    }

    pub fn secret_bindings(&self) -> &SecretBindings {
        &self.secret_bindings
    }

    pub fn platform(&self) -> &PlatformOptions {
        &self.platform
    }

    pub fn idle_timeout_seconds(&self) -> Option<NonZeroU32> {
        self.idle_timeout_seconds
    }

    pub fn no_ttl(&self) -> LeaseTtl {
        self.no_ttl
    }

    pub fn idle_policy(&self) -> Option<&IdlePolicy> {
        self.idle_policy.as_ref()
    }

    /// The CPU allotment in thousandths of a vCPU, or `None` for the provider default.
    pub fn vcpu_millis(&self) -> Option<u32> {
        self.vcpu_millis
    }

    /// The writable disk quota in MiB, or `None` for the provider default.
    pub fn disk_mib(&self) -> Option<u32> {
        self.disk_mib
    }

    pub fn retention(&self) -> RetentionToggles {
        self.retention
    }
}

fn check_env(env: &BTreeMap<String, String>) -> Result<(), SandboxSpecError> {
    if env.len() > SandboxSpec::MAX_ENV_ENTRIES {
        return Err(SandboxSpecError::TooManyEnvEntries {
            max: SandboxSpec::MAX_ENV_ENTRIES,
            got: env.len(),
        });
    }
    let mut total = 0usize;
    for (key, value) in env {
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            return Err(SandboxSpecError::InvalidEnvKey { key: key.clone() });
        }
        if key.len() > SandboxSpec::MAX_ENV_KEY_BYTES {
            return Err(SandboxSpecError::EnvKeyTooLong {
                key: key.clone(),
                max: SandboxSpec::MAX_ENV_KEY_BYTES,
            });
        }
        if value.len() > SandboxSpec::MAX_ENV_VALUE_BYTES || value.contains('\0') {
            return Err(SandboxSpecError::InvalidEnvValue {
                key: key.clone(),
                max: SandboxSpec::MAX_ENV_VALUE_BYTES,
            });
        }
        total += key.len() + value.len();
    }
    if total > SandboxSpec::MAX_ENV_TOTAL_BYTES {
        return Err(SandboxSpecError::EnvTooLarge {
            total,
            max: SandboxSpec::MAX_ENV_TOTAL_BYTES,
        });
    }
    Ok(())
}

fn check_idle(parts: &SandboxSpecParts) -> Result<(), SandboxSpecError> {
    let max = SandboxSpec::MAX_IDLE_SECONDS;
    if parts.idle_timeout_seconds.is_some_and(|idle| idle.get() > max) {
        return Err(SandboxSpecError::IdleTimeoutTooLong { max });
    }
    if let Some(policy) = &parts.idle_policy {
        if parts.idle_timeout_seconds.is_some() {
            return Err(SandboxSpecError::IdleTimeoutWithPolicy);
        }
        if !policy.fits_within(max) {
            return Err(SandboxSpecError::IdlePolicyTooLong { max });
        }
    }
    if parts.no_ttl == LeaseTtl::Unbounded {
        let idle_release = parts
            .idle_policy
            .as_ref()
            .is_some_and(|policy| policy.stages().iter().any(|stage| stage.action == IdleAction::Release));
        if parts.idle_timeout_seconds.is_none() && !idle_release {
            return Err(SandboxSpecError::NoTtlWithoutIdleRelease);
        }
    }
    Ok(())
}

fn check_drives(drives: &[DriveMount]) -> Result<(), SandboxSpecError> {
    if drives.len() > SandboxSpec::MAX_DRIVES {
        return Err(SandboxSpecError::Drives(DriveError::TooManyDrives {
            count: drives.len(),
            max: SandboxSpec::MAX_DRIVES,
        }));
    }
    let mut seen = HashSet::new();
    for drive in drives {
        if !seen.insert(&drive.mount_path) {
            return Err(SandboxSpecError::Drives(DriveError::DriveMountPathOverlap {
                mount_path: drive.mount_path.to_string(),
            }));
        }
    }
    Ok(())
}

fn check_exposed_ports(ports: &[Port]) -> Result<(), SandboxSpecError> {
    if ports.len() > SandboxSpec::MAX_EXPOSED_PORTS {
        return Err(SandboxSpecError::TooManyExposedPorts {
            max: SandboxSpec::MAX_EXPOSED_PORTS,
            got: ports.len(),
        });
    }
    let mut seen = HashSet::new();
    for port in ports {
        if !seen.insert(port) {
            return Err(SandboxSpecError::DuplicateExposedPort(*port));
        }
    }
    Ok(())
}

fn check_secret_bindings(
    bindings: &SecretBindings,
    env: &BTreeMap<String, String>,
    network_policy: Option<&NetworkPolicy>,
) -> Result<(), SandboxSpecError> {
    for binding in bindings.bindings() {
        let name = binding.name();
        if let SecretTarget::Env(var) = binding.target()
            && env.contains_key(var.as_str())
        {
            return Err(SandboxSpecError::EnvCollision {
                name: name.clone(),
                var: var.clone(),
            });
        }
        let Some(NetworkPolicy::UserDefined {
            rules, denied_domains, ..
        }) = network_policy
        else {
            return Err(SandboxSpecError::NoAllowedDomains(name.clone()));
        };
        for declared in binding.inject_hosts().hosts() {
            let host = binding.inject_scope().effective_pattern(declared);
            if let Some(denied) = denied_domains
                .iter()
                .find(|denied| host.is_covered_by(denied.as_pattern()))
            {
                return Err(SandboxSpecError::InjectHostDenied {
                    name: name.clone(),
                    host,
                    denied: denied.clone(),
                });
            }
            if !rules.iter().any(|rule| host.is_covered_by(&rule.domain.without_port())) {
                return Err(SandboxSpecError::InjectHostNotAllowed {
                    name: name.clone(),
                    host,
                });
            }
        }
    }
    Ok(())
}

impl fmt::Debug for SandboxSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env: BTreeMap<&String, &str> = self.env.keys().map(|key| (key, "[REDACTED]")).collect();
        formatter
            .debug_struct("SandboxSpec")
            .field("image", &self.image)
            .field("workspace", &self.workspace)
            .field("env", &env)
            .field("cpu_seconds", &self.cpu_seconds)
            .field("memory_mib", &self.memory_mib)
            .field("snapshot_out", &self.snapshot_out)
            .field("network_policy", &self.network_policy)
            .field("mounts", &self.mounts)
            .field("drives", &self.drives)
            .field("exposed_ports", &self.exposed_ports)
            .field("boot_mode", &self.boot_mode)
            .field("working_dir", &self.working_dir)
            .field("user", &self.user)
            .field("privileged", &self.privileged)
            .field("wasi_capabilities", &self.wasi_capabilities)
            .field("fs_policy", &self.fs_policy)
            .field("secret_bindings", &self.secret_bindings)
            .field("platform", &self.platform)
            .field("idle_timeout_seconds", &self.idle_timeout_seconds)
            .field("no_ttl", &self.no_ttl)
            .field("idle_policy", &self.idle_policy)
            .field("vcpu_millis", &self.vcpu_millis)
            .field("disk_mib", &self.disk_mib)
            .field("retention", &self.retention)
            .finish()
    }
}

impl TryFrom<SandboxSpecWire> for SandboxSpec {
    type Error = SandboxSpecError;

    fn try_from(wire: SandboxSpecWire) -> Result<Self, Self::Error> {
        Self::new(SandboxSpecParts {
            image: wire.image,
            workspace: wire.workspace,
            env: wire.env,
            cpu_seconds: wire.cpu_seconds,
            memory_mib: wire.memory_mib,
            snapshot_out: wire.snapshot_out,
            network_policy: wire.network_policy,
            mounts: wire.mounts,
            drives: wire.drives,
            exposed_ports: wire.exposed_ports,
            boot_mode: wire.boot_mode,
            working_dir: wire.working_dir,
            user: wire.user,
            privileged: wire.privileged,
            wasi_capabilities: wire.wasi_capabilities,
            fs_policy: wire.fs_policy,
            secret_bindings: wire.secret_bindings,
            platform: wire.platform,
            idle_timeout_seconds: wire.idle_timeout_seconds,
            no_ttl: wire.no_ttl,
            idle_policy: wire.idle_policy,
            vcpu_millis: wire.vcpu_millis,
            disk_mib: wire.disk_mib,
            retention: wire.retention,
        })
    }
}

impl From<SandboxSpec> for SandboxSpecWire {
    fn from(spec: SandboxSpec) -> Self {
        Self {
            image: spec.image,
            workspace: spec.workspace,
            env: spec.env,
            cpu_seconds: spec.cpu_seconds,
            memory_mib: spec.memory_mib,
            snapshot_out: spec.snapshot_out,
            network_policy: spec.network_policy,
            mounts: spec.mounts,
            drives: spec.drives,
            exposed_ports: spec.exposed_ports,
            boot_mode: spec.boot_mode,
            working_dir: spec.working_dir,
            user: spec.user,
            privileged: spec.privileged,
            wasi_capabilities: spec.wasi_capabilities,
            fs_policy: spec.fs_policy,
            secret_bindings: spec.secret_bindings,
            platform: spec.platform,
            idle_timeout_seconds: spec.idle_timeout_seconds,
            no_ttl: spec.no_ttl,
            idle_policy: spec.idle_policy,
            vcpu_millis: spec.vcpu_millis,
            disk_mib: spec.disk_mib,
            retention: spec.retention,
        }
    }
}
