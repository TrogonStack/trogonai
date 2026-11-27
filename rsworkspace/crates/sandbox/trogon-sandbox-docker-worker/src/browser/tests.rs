use std::error::Error;
use std::sync::{Mutex, MutexGuard, PoisonError};

use base64::Engine;
use trogon_sandbox_contract::{
    CapabilityNeed, CapabilityRequirements, ExtraCapability, PidsLimit, SandboxImage, SdkFeature, ShmSizeMib,
};

use super::*;
use crate::support_tests::{TestResult, lease_request_with, spec_parts};

const IMAGE: &str = "registry.example.test/sandbox/browser:node22";
const IMAGE_ID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jWZkAAAAASUVORK5CYII=";

fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Runtime {
    calls: Mutex<Vec<String>>,
    specs: Mutex<Vec<BrowserProbeSpec>>,
    reply: serde_json::Value,
    fail_probe: bool,
    hang_probe: bool,
    fail_remove: bool,
}

impl Runtime {
    fn valid() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            specs: Mutex::new(Vec::new()),
            reply: serde_json::json!({"node": "22.23.4", "uid": 65532, "home": "/tmp", "png": PNG, "temporary_removed": true}),
            fail_probe: false,
            hang_probe: false,
            fail_remove: false,
        }
    }

    fn last_call(&self) -> Option<String> {
        held(&self.calls).last().cloned()
    }
}

#[async_trait]
impl BrowserProbeRuntime for Runtime {
    async fn image_id(&self, reference: &str) -> anyhow::Result<String> {
        held(&self.calls).push(format!("inspect:{reference}"));
        Ok(IMAGE_ID.into())
    }

    async fn probe(&self, spec: &BrowserProbeSpec) -> anyhow::Result<BrowserProbeOutput> {
        held(&self.calls).push("probe".into());
        held(&self.specs).push(spec.clone());
        if self.hang_probe {
            std::future::pending::<()>().await;
        }
        if self.fail_probe {
            anyhow::bail!("primary screenshot failure")
        }
        Ok(BrowserProbeOutput {
            exit_code: 0,
            stdout: serde_json::to_vec(&self.reply)?,
            stderr: Vec::new(),
        })
    }

    async fn remove(&self, name: &str) -> anyhow::Result<()> {
        held(&self.calls).push(format!("remove:{name}"));
        if self.fail_remove {
            anyhow::bail!("probe cleanup failed")
        }
        Ok(())
    }
}

fn registry_image(reference: &str) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(reference),
        pull_secret: None,
    }
}

fn parts_for_browser() -> Result<trogon_sandbox_contract::SandboxSpecParts, Box<dyn Error>> {
    let mut parts = spec_parts()?;
    parts.image = registry_image(IMAGE);
    parts.memory_mib = std::num::NonZeroU32::new(1024).ok_or("memory")?;
    parts.vcpu_millis = Some(1000);
    Ok(parts)
}

fn request_from(
    parts: trogon_sandbox_contract::SandboxSpecParts,
    browser: bool,
) -> Result<LeaseRequest, Box<dyn Error>> {
    let mut request = lease_request_with("browser-test", trogon_sandbox_contract::SandboxSpec::new(parts)?)?;
    request.required_capabilities = Some(CapabilityRequirements {
        needs_browser: CapabilityNeed::from(browser),
        ..Default::default()
    });
    Ok(request)
}

fn request(browser: bool) -> Result<LeaseRequest, Box<dyn Error>> {
    request_from(parts_for_browser()?, browser)
}

fn verified() -> BrowserSupport {
    BrowserSupport {
        reference: OciImageRef::parse_unqualified(IMAGE),
        image_id: IMAGE_ID.into(),
    }
}

fn admitted(request: &LeaseRequest) -> bool {
    admit_browser(request, Some(&verified()), &ContainerHardening::secure()).is_ok()
}

#[tokio::test]
async fn browser_probe_runs_exact_immutable_content_with_bounded_nonroot_resources_and_cleanup() -> TestResult {
    let runtime = Runtime::valid();
    let support = verify_browser(
        &runtime,
        OciImageRef::parse_unqualified(IMAGE),
        &ContainerHardening::secure(),
    )
    .await?;
    assert_eq!(support.image_id, IMAGE_ID);
    let specs = held(&runtime.specs);
    let spec = specs.first().ok_or("no probe spec")?;
    assert_eq!(spec.container.image, IMAGE_ID);
    assert_eq!(spec.container.user.as_deref(), Some("65532:65532"));
    assert_eq!(spec.container.network_mode, "none");
    assert!(spec.container.env.contains(&"HOME=/tmp".into()));
    assert_eq!(spec.container.memory_mib, 512);
    assert_eq!(spec.container.cpu, 1.0);
    assert_eq!(spec.container.hardening.pids_limit, Some(512));
    assert_eq!(spec.container.hardening.shm_size_mib, Some(256));
    assert!(spec.container.hardening.no_new_privileges);
    assert!(spec.container.hardening.drop_all_capabilities);
    assert!(spec.container.hardening.disable_swap);
    assert!(spec.container.cap_add.is_empty());
    assert_eq!(spec.container.cap_drop, vec!["ALL".to_string()]);
    assert!(spec.timeout <= Duration::from_secs(20));
    assert_eq!(spec.command.first().map(String::as_str), Some("node"));
    let calls = held(&runtime.calls);
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0], format!("inspect:{IMAGE}"));
    assert_eq!(calls[1], "probe");
    assert_eq!(calls[2], format!("remove:{}", spec.container.name));
    Ok(())
}

#[tokio::test]
async fn browser_probe_refuses_old_node_missing_png_root_uid_wrong_home_and_leftovers() -> TestResult {
    for (field, value) in [
        ("node", serde_json::json!("18.20.8")),
        ("node", serde_json::json!("22.6.0")),
        ("node", serde_json::json!("not-a-version")),
        ("uid", serde_json::json!(0)),
        ("home", serde_json::json!("/root")),
        ("temporary_removed", serde_json::json!(false)),
        (
            "png",
            serde_json::json!(base64::engine::general_purpose::STANDARD.encode(b"not an image")),
        ),
    ] {
        let mut runtime = Runtime::valid();
        runtime.reply[field] = value;
        let result = verify_browser(
            &runtime,
            OciImageRef::parse_unqualified(IMAGE),
            &ContainerHardening::secure(),
        )
        .await;
        assert!(result.is_err());
        let last = runtime.last_call().ok_or("no calls")?;
        assert!(last.starts_with("remove:"));
    }
    Ok(())
}

#[tokio::test]
async fn browser_probe_preserves_primary_failure_and_cleans_only_its_container() -> TestResult {
    let mut runtime = Runtime::valid();
    runtime.fail_probe = true;
    runtime.fail_remove = true;
    let error = verify_browser(
        &runtime,
        OciImageRef::parse_unqualified(IMAGE),
        &ContainerHardening::secure(),
    )
    .await
    .err()
    .ok_or("probe should fail")?;
    let message = format!("{error:#}");
    assert!(message.contains("primary screenshot failure"), "{message}");
    assert!(message.contains("probe cleanup failed"), "{message}");
    assert_eq!(
        held(&runtime.calls)
            .iter()
            .filter(|call| call.starts_with("remove:"))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn browser_probe_timeout_still_removes_its_named_container() -> TestResult {
    let mut runtime = Runtime::valid();
    runtime.hang_probe = true;
    let result = verify_browser(
        &runtime,
        OciImageRef::parse_unqualified(IMAGE),
        &ContainerHardening::secure(),
    )
    .await;
    assert!(result.is_err());
    let last = runtime.last_call().ok_or("no calls")?;
    assert!(last.starts_with("remove:"));
    Ok(())
}

#[test]
fn browser_admission_requires_verified_exact_image_and_preserves_regular_leases() -> TestResult {
    assert!(admit_browser(&request(true)?, None, &ContainerHardening::secure()).is_err());
    let mut parts = parts_for_browser()?;
    parts.image = registry_image("registry.example.test/other:latest");
    assert!(!admitted(&request_from(parts, true)?));
    let mut parts = parts_for_browser()?;
    parts.image = registry_image("");
    assert!(
        trogon_sandbox_contract::SandboxSpec::new(parts).is_err(),
        "a browser lease with no image never reaches admission"
    );
    let regular = admit_browser(&request(false)?, Some(&verified()), &ContainerHardening::secure())?;
    assert!(regular.is_none());
    Ok(())
}

#[test]
fn browser_admission_uses_probed_image_id_even_if_the_configured_tag_moves() -> TestResult {
    let req = request(true)?;
    let launch = admit_browser(&req, Some(&verified()), &ContainerHardening::secure())?.ok_or("browser launch")?;
    assert_eq!(launch.image_id, IMAGE_ID);
    assert_eq!(launch.user, "65532:65532");
    assert_eq!(launch.env.get("HOME").map(String::as_str), Some("/tmp"));
    assert_eq!(req.spec.image(), &registry_image(IMAGE));
    Ok(())
}

#[test]
fn browser_admission_refuses_memory_outside_the_bounds() -> TestResult {
    for memory in [128, 4097] {
        let mut parts = parts_for_browser()?;
        parts.memory_mib = std::num::NonZeroU32::new(memory).ok_or("memory")?;
        assert!(!admitted(&request_from(parts, true)?));
    }
    Ok(())
}

#[test]
fn browser_admission_refuses_cpu_user_and_home_mismatches() -> TestResult {
    let mut parts = parts_for_browser()?;
    parts.vcpu_millis = Some(4001);
    assert!(!admitted(&request_from(parts, true)?));
    let mut parts = parts_for_browser()?;
    parts.user = Some("0:0".into());
    assert!(!admitted(&request_from(parts, true)?));
    let mut parts = parts_for_browser()?;
    parts.env.insert("HOME".into(), "/root".into());
    assert!(!admitted(&request_from(parts, true)?));
    Ok(())
}

#[test]
fn browser_admission_refuses_privilege_and_hardening_mismatches() -> TestResult {
    let mut parts = parts_for_browser()?;
    parts.privileged = trogon_sandbox_contract::PrivilegedAccess::Granted;
    assert!(!admitted(&request_from(parts, true)?));
    let mut parts = parts_for_browser()?;
    parts.platform.container.shm_size_mib = Some(ShmSizeMib::new(16)?);
    assert!(!admitted(&request_from(parts, true)?));
    let mut parts = parts_for_browser()?;
    parts.platform.container.pids_limit = Some(PidsLimit::new(4096)?);
    assert!(!admitted(&request_from(parts, true)?));
    let mut parts = parts_for_browser()?;
    parts
        .platform
        .container
        .extra_capabilities
        .push(ExtraCapability::new("NET_RAW")?);
    assert!(!admitted(&request_from(parts, true)?));
    for native in [
        serde_json::json!({"local_docker": {"shm_size_mib": 16}}),
        serde_json::json!({"local_docker": {"pids_limit": 4096}}),
    ] {
        let mut req = request(true)?;
        req.native = Some(native);
        assert!(!admitted(&req));
    }
    Ok(())
}

#[test]
fn browser_profile_is_earned_only_by_verified_runtime_support() -> TestResult {
    let resources = crate::resources(4, None);
    assert!(
        !profile_with_browser(resources, None)
            .isolation
            .supports_browser
            .is_supported()
    );
    let profile = profile_with_browser(resources, Some(&verified()));
    assert!(profile.isolation.supports_browser.is_supported());
    assert!(profile.sdk_features().contains(&SdkFeature::Browser));
    assert_eq!(profile.resources, resources);
    Ok(())
}
