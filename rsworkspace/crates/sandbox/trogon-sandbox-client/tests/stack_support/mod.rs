//! Helpers shared by the live-stack lanes.
//!
//! Every lane that uses them reaches a running broker over NATS, so each of its tests is `#[ignore]`d and runs only
//! when asked for with `--ignored`. `SANDBOX_NATS_URL` names the broker's NATS server. Without it the lanes use
//! `nats://127.0.0.1:<SANDBOX_NATS_PORT>`, the port the local compose stack in `devops/docker/compose/sandbox`
//! publishes (default 24222).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::{SystemTime, UNIX_EPOCH};

use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{
    BootMode, CommandOutput, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, LeaseRequestKeyError,
    LeaseTtl, OciImageRef, PlatformOptions, PrivilegedAccess, ProviderKind, RetentionToggles, SandboxImage,
    SandboxSpec, SandboxSpecParts, SandboxTags, SecretBindings, TenantId, TenantIdError, WorkspaceSource,
};
use trogon_std::env::{ReadEnv, SystemEnv};

/// The NATS port the local compose stack publishes when `SANDBOX_NATS_PORT` is unset.
pub const DEFAULT_NATS_PORT: &str = "24222";

/// The tenant the smoke lanes lease under when `SANDBOX_SMOKE_TENANT` is unset.
pub const DEFAULT_SMOKE_TENANT: &str = "smoke-tenant";

/// The value of `name` in the process environment.
pub fn var(name: &str) -> Result<String, std::env::VarError> {
    SystemEnv.var(name)
}

/// The value of `name`, or `default` when it is unset or not valid UTF-8.
pub fn env_or(name: &str, default: &str) -> String {
    var(name).unwrap_or_else(|_| default.to_string())
}

/// The value of `name`, or an error naming it when it is unset, so a lane that cannot run fails instead of
/// passing by skipping.
pub fn required_env(name: &str) -> Result<String, String> {
    var(name).map_err(|_| format!("set {name} to run this lane"))
}

/// The broker's NATS URL.
pub fn nats_url() -> String {
    var("SANDBOX_NATS_URL")
        .unwrap_or_else(|_| format!("nats://127.0.0.1:{}", env_or("SANDBOX_NATS_PORT", DEFAULT_NATS_PORT)))
}

/// A client connected to the broker's NATS server.
pub async fn connect() -> Result<SandboxClient, Box<dyn std::error::Error>> {
    let url = nats_url();
    SandboxClient::connect(&url)
        .await
        .map_err(|error| format!("connect to {url}: {error}").into())
}

/// The tenant the smoke lanes lease under, from `SANDBOX_SMOKE_TENANT`.
pub fn smoke_tenant() -> Result<TenantId, TenantIdError> {
    TenantId::new(env_or("SANDBOX_SMOKE_TENANT", DEFAULT_SMOKE_TENANT))
}

/// `label` followed by the current time in nanoseconds, unique per call for any one lane.
pub fn unique(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("{label}-{nanos}")
}

/// A fresh idempotency key, so every lease a lane asks for is a new one.
pub fn request_key(label: &str) -> Result<LeaseRequestKey, LeaseRequestKeyError> {
    LeaseRequestKey::new(unique(label))
}

/// A registry image without a pull secret.
pub fn registry_image(reference: &str) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(reference),
        pull_secret: None,
    }
}

/// A spec for `image` with an empty workspace and every optional field left at its default. Change the fields a
/// lane cares about, then check them with [`SandboxSpec::new`].
pub fn spec_parts(
    image: SandboxImage,
    cpu_seconds: u32,
    memory_mib: u32,
) -> Result<SandboxSpecParts, Box<dyn std::error::Error>> {
    Ok(SandboxSpecParts {
        image,
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(cpu_seconds).ok_or("cpu_seconds must not be zero")?,
        memory_mib: NonZeroU32::new(memory_mib).ok_or("memory_mib must not be zero")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: SecretBindings::default(),
        platform: PlatformOptions::default(),
        idle_timeout_seconds: None,
        no_ttl: LeaseTtl::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: RetentionToggles::default(),
    })
}

/// A persistent lease of `spec` on `provider`, with a fresh idempotency key built from `label`.
pub fn lease_request(
    tenant: TenantId,
    provider: ProviderKind,
    spec: SandboxSpec,
    ttl_seconds: u32,
    label: &str,
) -> Result<LeaseRequest, LeaseRequestKeyError> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant,
        provider_pref: vec![provider],
        spec,
        ttl_seconds,
        idempotency_key: request_key(label)?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: LeasePrewarm::Regular,
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Persistent,
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

/// `/bin/sh -c script`.
pub fn sh(script: &str) -> Vec<String> {
    vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()]
}

/// A command's stdout as text.
pub fn stdout(output: &CommandOutput) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A command's stderr as text.
pub fn stderr(output: &CommandOutput) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
