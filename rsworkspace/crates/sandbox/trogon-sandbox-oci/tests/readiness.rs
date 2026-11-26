mod support;

use serde_json::json;
use support::{FakeRegistry, OCI_INDEX, TestResult, amd64_config, image_ref, put_image, sha256};
use trogon_sandbox_oci::{ImageReadiness, OciError};

fn index(entries: serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&json!({"schemaVersion": 2, "mediaType": OCI_INDEX, "manifests": entries}))
}

fn entry(architecture: &str) -> serde_json::Value {
    json!({"mediaType": support::OCI_MANIFEST, "digest": sha256(architecture.as_bytes()), "size": 1,
        "platform": {"os": "linux", "architecture": architecture}})
}

#[tokio::test]
async fn indexes_are_ready_only_with_a_linux_amd64_entry() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    registry.put_manifest(
        "acme/app",
        "both",
        OCI_INDEX,
        &index(json!([entry("arm64"), entry("amd64")]))?,
    );
    registry.put_manifest("acme/app", "arm", OCI_INDEX, &index(json!([entry("arm64")]))?);
    assert_eq!(
        client
            .resolve_image_readiness(&image_ref(&registry.address, "both"))
            .await?,
        ImageReadiness::Ready
    );
    let readiness = client
        .resolve_image_readiness(&image_ref(&registry.address, "arm"))
        .await?;
    assert!(matches!(readiness, ImageReadiness::Incompatible { reason } if reason.contains("1 platform")));
    Ok(())
}

#[tokio::test]
async fn single_manifests_are_judged_by_their_config() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    put_image(&registry, "acme/app", "amd", &amd64_config(), &[])?;
    let arm = json!({"architecture": "arm64", "os": "linux", "rootfs": {"type": "layers", "diff_ids": []}});
    put_image(&registry, "acme/app", "arm", &arm, &[])?;
    let junk = json!({"architecture": 5});
    put_image(&registry, "acme/app", "junk", &junk, &[])?;

    assert_eq!(
        client
            .resolve_image_readiness(&image_ref(&registry.address, "amd"))
            .await?,
        ImageReadiness::Ready
    );
    let readiness = client
        .resolve_image_readiness(&image_ref(&registry.address, "arm"))
        .await?;
    assert!(matches!(readiness, ImageReadiness::Incompatible { reason } if reason.contains("linux/arm64")));
    let error = client
        .resolve_image_readiness(&image_ref(&registry.address, "junk"))
        .await
        .err();
    assert!(matches!(error, Some(OciError::Layout(_))), "{error:?}");
    assert!(
        client
            .resolve_image_readiness(&image_ref(&registry.address, "absent"))
            .await
            .is_err()
    );
    Ok(())
}
