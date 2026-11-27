//! End to end: `SandboxClient` to the broker to `trogon-sandbox-hyperlight-worker` to a Wasm component in a
//! hypervisor-isolated guest.
//!
//! Needs NATS with JetStream and `trogon-sandbox-broker` on the same server (see `stack_support` for how the lane
//! finds it), and a hyperlight worker with a hypervisor backend whose component root
//! (`SANDBOX_HYPERLIGHT_COMPONENT_ROOT`) holds the component named by `SANDBOX_SMOKE_HYPERLIGHT_IMAGE` (default
//! `passthrough_guest`, which resolves to `passthrough_guest.wasm`). Only runs on a host with `/dev/kvm`,
//! `/dev/mshv` or WHP.

mod common;
mod stack_support;

use std::time::Duration;

use common::TestResult;
use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef, ProviderKind, SandboxImage, SandboxSpec};

#[tokio::test]
#[ignore = "requires a hypervisor host running the broker and a hyperlight worker with a hypervisor backend"]
async fn lease_wasm_guest_via_broker_and_hyperlight_worker() -> TestResult {
    let component = stack_support::env_or("SANDBOX_SMOKE_HYPERLIGHT_IMAGE", "passthrough_guest");
    let image = SandboxImage::HyperlightModule {
        reference: OciArtifactRef {
            registry: String::new(),
            repository: component,
            digest: Digest::new(format!("sha256:{}", "0".repeat(64)))?,
            artifact_type: ArtifactType::HyperlightSnapshotV1,
        },
    };
    let request = stack_support::lease_request(
        stack_support::smoke_tenant()?,
        ProviderKind::Hyperlight,
        SandboxSpec::new(stack_support::spec_parts(image, 30, 64)?)?,
        60,
        "smoke-hyperlight",
    )?;

    let client = stack_support::connect().await?;
    let granted = client.lease(&request, Duration::from_secs(45)).await?;
    let cancelled = client.cancel(&granted.lease_id).await;

    assert_eq!(granted.provider, ProviderKind::Hyperlight);
    assert!(!granted.sandbox_id.is_empty());
    assert!(!granted.evidence_subject.is_empty());
    cancelled?;
    Ok(())
}
