use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, ensure};
use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use trogon_sandbox_contract::{
    CapabilitySupport, Digest, LeaseRequest, OciImageRef, ProviderCapabilityProfile, ProviderKind, ResourceCapability,
    SandboxImage,
};
use trogon_sandbox_providers::ContainerHardening;
use trogon_sandbox_providers::ContainerRunSpec;

pub mod runtime;

mod constants;

use constants::{BROWSER_HOME, BROWSER_USER, CLEANUP_TIMEOUT, MAX_PIDS, MAX_SHM_MIB, PROBE_SCRIPT, PROBE_TIMEOUT};

#[derive(Clone, Debug)]
pub struct BrowserSupport {
    reference: OciImageRef,
    image_id: String,
}

#[derive(Clone, Debug)]
pub struct BrowserLaunch {
    pub image_id: String,
    pub user: String,
    pub env: BTreeMap<String, String>,
    pub hardening: ContainerHardening,
}

#[derive(Clone, Debug)]
pub struct BrowserProbeSpec {
    pub container: ContainerRunSpec,
    pub command: Vec<String>,
    pub timeout: Duration,
}

pub struct BrowserProbeOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[async_trait]
pub trait BrowserProbeRuntime: Send + Sync {
    async fn image_id(&self, reference: &str) -> anyhow::Result<String>;
    async fn probe(&self, spec: &BrowserProbeSpec) -> anyhow::Result<BrowserProbeOutput>;
    async fn remove(&self, name: &str) -> anyhow::Result<()>;
}

fn browser_hardening(base: &ContainerHardening) -> anyhow::Result<ContainerHardening> {
    let mut hardening = base.clone();
    let pids = base
        .pids_limit
        .filter(|value| *value > 0)
        .unwrap_or(MAX_PIDS)
        .min(MAX_PIDS);
    let shm = base.shm_size_mib.unwrap_or(256).min(MAX_SHM_MIB);
    ensure!(pids >= 128, "browser workloads require a pids limit of at least 128");
    ensure!(
        shm >= 128,
        "browser workloads require at least 128 MiB of shared memory"
    );
    hardening.pids_limit = Some(pids);
    hardening.shm_size_mib = Some(shm);
    hardening.no_new_privileges = true;
    hardening.drop_all_capabilities = true;
    hardening.retained_capabilities.clear();
    hardening.disable_swap = true;
    Ok(hardening)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeReceipt {
    node: String,
    uid: u32,
    home: String,
    png: String,
    temporary_removed: bool,
}

fn validate_probe(output: BrowserProbeOutput) -> anyhow::Result<()> {
    ensure!(
        output.exit_code == 0,
        "browser screenshot probe failed (exit {})",
        output.exit_code
    );
    let receipt: ProbeReceipt = serde_json::from_slice(&output.stdout).context("invalid browser probe receipt")?;
    let parts: Vec<_> = receipt
        .node
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<_, _>>()
        .context("invalid browser Node version")?;
    ensure!(
        parts.len() == 3 && (parts[0] > 22 || (parts[0] == 22 && parts[1] >= 7)),
        "browser image needs Node 22.7 or newer"
    );
    ensure!(receipt.uid == 65532, "browser probe must run as UID 65532");
    ensure!(receipt.home == BROWSER_HOME, "browser probe needs writable HOME=/tmp");
    ensure!(receipt.temporary_removed, "browser probe left temporary files behind");
    let png = base64::engine::general_purpose::STANDARD
        .decode(receipt.png)
        .context("invalid browser screenshot encoding")?;
    ensure!(
        png.len() > 64 && png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "browser probe produced no PNG screenshot"
    );
    Ok(())
}

pub async fn verify_browser(
    runtime: &dyn BrowserProbeRuntime,
    reference: OciImageRef,
    hardening: &ContainerHardening,
) -> anyhow::Result<BrowserSupport> {
    ensure!(
        !reference.repository.trim().is_empty() && (reference.tag.is_some() || reference.digest.is_some()),
        "browser image needs an explicit repository and tag or digest"
    );
    let hardening = browser_hardening(hardening)?;
    let image_id = tokio::time::timeout(CLEANUP_TIMEOUT, runtime.image_id(&reference.allowlist_string()))
        .await
        .context("browser image inspection timed out")??;
    Digest::new(image_id.clone()).context("browser image has no immutable Docker identity")?;
    let name = format!("sandbox-browser-probe-{}", crate::fresh_id::fresh_id());
    let spec = BrowserProbeSpec {
        container: ContainerRunSpec {
            name: name.clone(),
            image: image_id.clone(),
            memory_mib: 512,
            cpu: 1.0,
            disk_mib: None,
            env: vec![format!("HOME={BROWSER_HOME}")],
            binds: Vec::new(),
            network_mode: "none".into(),
            entrypoint_override: Some(Vec::new()),
            cmd_override: Some(vec!["sleep".into(), "infinity".into()]),
            working_dir: Some(BROWSER_HOME.into()),
            user: Some(BROWSER_USER.into()),
            cap_add: Vec::new(),
            cap_drop: vec!["ALL".into()],
            exposed_ports: Vec::new(),
            hardening,
            labels: BTreeMap::new(),
        },
        command: vec![
            "node".into(),
            "--input-type=module".into(),
            "-e".into(),
            PROBE_SCRIPT.into(),
        ],
        timeout: PROBE_TIMEOUT,
    };
    let result = match tokio::time::timeout(PROBE_TIMEOUT, runtime.probe(&spec)).await {
        Ok(output) => output.and_then(validate_probe),
        Err(_) => Err(anyhow::anyhow!("browser probe timed out")),
    };
    let cleanup = tokio::time::timeout(CLEANUP_TIMEOUT, runtime.remove(&name))
        .await
        .context("browser probe cleanup timed out")
        .and_then(|result| result);
    match (result, cleanup) {
        (Err(primary), Err(cleanup)) => {
            return Err(primary.context(format!("browser probe cleanup also failed: {cleanup:#}")));
        }
        (Err(primary), Ok(())) => return Err(primary),
        (Ok(()), Err(cleanup)) => return Err(cleanup.context("remove browser probe container")),
        (Ok(()), Ok(())) => {}
    }
    Ok(BrowserSupport { reference, image_id })
}

pub fn admit_browser(
    request: &LeaseRequest,
    support: Option<&BrowserSupport>,
    hardening: &ContainerHardening,
) -> anyhow::Result<Option<BrowserLaunch>> {
    if !request
        .required_capabilities
        .as_ref()
        .is_some_and(|requirements| requirements.needs_browser.is_required())
    {
        return Ok(None);
    }
    let support = support.context("browser request requires an operator-configured verified browser image")?;
    ensure!(
        matches!(request.spec.image(), SandboxImage::Registry { reference, .. } if reference == &support.reference),
        "browser request image must match the worker's verified browser image"
    );
    ensure!(
        (512..=4096).contains(&request.spec.memory_mib().get()),
        "browser memory must be between 512 and 4096 MiB"
    );
    ensure!(
        (1000..=4000).contains(&request.spec.vcpu_millis().unwrap_or(1000)),
        "browser CPU must be between 1000 and 4000 millicores"
    );
    ensure!(
        !request.spec.privileged().is_granted(),
        "browser workloads cannot be privileged"
    );
    ensure!(
        request
            .spec
            .user()
            .is_none_or(|user| matches!(user, "65532" | BROWSER_USER)),
        "browser workloads must use UID 65532"
    );
    ensure!(
        request.spec.env().get("HOME").is_none_or(|home| home == BROWSER_HOME),
        "browser workloads need HOME=/tmp"
    );
    let overrides = &request.spec.platform().container;
    ensure!(
        overrides.extra_capabilities.is_empty(),
        "browser workloads cannot add container capabilities"
    );
    let native = trogon_sandbox_protocol::options_for(ProviderKind::LocalDocker, request.native.as_ref())?;
    let mut applied = crate::hardening_overrides::hardening_with_overrides(hardening, overrides);
    if let Some(native) = crate::adapter::hardening_with_native(&applied, request.native.as_ref())? {
        applied = native;
    }
    if overrides.pids_limit.is_some() || native.contains_key("pids_limit") {
        ensure!(
            applied.pids_limit.is_some_and(|pids| (128..=MAX_PIDS).contains(&pids)),
            "browser pids limit must be between 128 and 512"
        );
    }
    if overrides.shm_size_mib.is_some() || native.contains_key("shm_size_mib") {
        ensure!(
            applied
                .shm_size_mib
                .is_some_and(|shm| (128..=MAX_SHM_MIB).contains(&shm)),
            "browser shared memory must be between 128 and 512 MiB"
        );
    }
    Ok(Some(BrowserLaunch {
        image_id: support.image_id.clone(),
        user: BROWSER_USER.into(),
        env: BTreeMap::from([("HOME".into(), BROWSER_HOME.into())]),
        hardening: browser_hardening(&applied)?,
    }))
}

pub fn profile_with_browser(
    resources: ResourceCapability,
    support: Option<&BrowserSupport>,
) -> ProviderCapabilityProfile {
    let mut profile = crate::profile_with_resources(resources);
    profile.isolation.supports_browser = CapabilitySupport::from(support.is_some());
    profile
}

#[cfg(test)]
mod tests;
