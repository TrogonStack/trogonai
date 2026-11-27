use serde_json::json;
use trogon_sandbox_contract::{ArtifactType, Digest};

use super::*;
use crate::test_fixtures::{TestResult, lease_json};

fn snapshot() -> Result<OciArtifactRef, Box<dyn std::error::Error>> {
    Ok(OciArtifactRef {
        registry: "registry.example".to_owned(),
        repository: "snapshots/one".to_owned(),
        digest: Digest::new(format!("sha256:{}", "a".repeat(64)))?,
        artifact_type: ArtifactType::SandboxRootfsV1,
    })
}

fn request_on(image: serde_json::Value) -> Result<LeaseRequest, serde_json::Error> {
    let mut body = lease_json(false);
    body["spec"]["image"] = image;
    serde_json::from_value(body)
}

#[test]
fn a_registry_image_becomes_the_snapshot_parent() -> TestResult {
    let rebuilt = request_with_snapshot(&request_on(lease_json(false)["spec"]["image"].clone())?, &snapshot()?)?;
    let SandboxImage::Snapshot { reference, parent } = rebuilt.spec.image() else {
        return Err("the image must be the snapshot".into());
    };
    assert_eq!(reference, &snapshot()?);
    assert_eq!(
        parent.as_ref().map(|image| image.repository.as_str()),
        Some("sessions/python")
    );
    Ok(())
}

#[test]
fn an_image_without_a_registry_origin_leaves_the_snapshot_parentless() -> TestResult {
    let image = json!({"kind": "snapshot_tag", "tag": {"tenant": "acme", "name": "nightly"}, "artifact_type": "sandbox_rootfs_v1"});
    let rebuilt = request_with_snapshot(&request_on(image)?, &snapshot()?)?;
    let SandboxImage::Snapshot { parent, .. } = rebuilt.spec.image() else {
        return Err("the image must be the snapshot".into());
    };
    assert_eq!(parent, &None);
    Ok(())
}
