mod support;

use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::json;
use support::{Rig, TestResult, artifact_ref, empty_workspace, lease, request, request_with, sha256};
use trogon_sandbox_contract::{ArtifactType, LeaseRequest};
use trogon_sandbox_hyperlight_worker::{
    HYPERLIGHT_SNAPSHOT_MEDIA_TYPE, HyperlightError, push_snapshot_artifact, snapshot_layer,
};
use trogon_sandbox_oci::build_push_target;
use trogon_sandbox_protocol::{Provider, ProviderError};

fn restore_request(registry: &str, repository: &str, digest: &str) -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    request_with(
        json!({"kind": "snapshot", "reference": {
            "registry": registry,
            "repository": repository,
            "digest": digest,
            "artifact_type": "hyperlight_snapshot_v1",
        }}),
        empty_workspace(),
        json!({}),
    )
}

async fn refusal(rig: &Rig, request: LeaseRequest) -> Result<String, Box<dyn std::error::Error>> {
    let refused = rig.provider.lease(&request, &lease("restored")).await.err();
    Ok(refused.ok_or("expected a refusal")?.to_string())
}

async fn assert_refused(rig: &Rig, request: LeaseRequest, expected: &str) -> TestResult {
    let message = refusal(rig, request).await?;
    assert!(message.contains(expected), "{message} lacks {expected}");
    Ok(())
}

#[tokio::test]
async fn publishes_a_snapshot_and_restores_it() -> TestResult {
    let rig = Rig::start().await?;
    rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    let receipt = rig.provider.snapshot(&lease("l1"), &rig.target("snaps")?).await?;
    assert_eq!(receipt.artifact_type, ArtifactType::HyperlightSnapshotV1);
    assert!(receipt.bytes > 0);

    let granted = rig
        .provider
        .lease(
            &restore_request(&rig.registry.address, "snaps", receipt.digest.as_str())?,
            &lease("restored"),
        )
        .await?;
    assert_eq!(granted.sandbox_id.as_str(), "hl-restored");
    assert_eq!(rig.hypervisor.loads(), 2);
    Ok(())
}

#[tokio::test]
async fn publishes_to_the_default_registry_when_the_target_names_none() -> TestResult {
    let rig = Rig::start().await?;
    rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    let mut target = rig.target("snaps")?;
    target.registry = String::new();
    let receipt = rig.provider.checkpoint_running(&lease("l1"), &target).await?;
    let restore = restore_request(&rig.registry.address, "snaps", receipt.digest.as_str())?;
    rig.provider.lease(&restore, &lease("restored")).await?;
    Ok(())
}

#[tokio::test]
async fn refuses_to_publish_what_it_cannot_snapshot() -> TestResult {
    let rig = Rig::start().await?;
    let digest = sha256(b"x");
    let wrong = artifact_ref(&rig.registry.address, "snaps", &digest, ArtifactType::SandboxRootfsV1)?;
    assert!(rig.provider.snapshot(&lease("l1"), &wrong).await.is_err());
    let none = rig.provider.snapshot(&lease("l1"), &rig.target("snaps")?).await;
    assert!(matches!(
        &none,
        Err(ProviderError::Backend(source))
            if matches!(source.downcast_ref::<HyperlightError>(), Some(HyperlightError::NoLease(id)) if id == "l1")
    ));

    rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    rig.hypervisor.fail_save(true);
    assert!(
        rig.provider
            .snapshot(&lease("l1"), &rig.target("snaps")?)
            .await
            .is_err()
    );
    rig.hypervisor.fail_save(false);
    let mut elsewhere = rig.target("snaps")?;
    elsewhere.registry = "127.0.0.1:1".into();
    assert!(rig.provider.snapshot(&lease("l1"), &elsewhere).await.is_err());
    let mut malformed = rig.target("snaps")?;
    malformed.registry = "not a registry".into();
    assert!(rig.provider.snapshot(&lease("l1"), &malformed).await.is_err());
    Ok(())
}

#[tokio::test]
async fn refuses_a_snapshot_it_cannot_restore() -> TestResult {
    let rig = Rig::start().await?;
    let missing = restore_request(&rig.registry.address, "nothing", &sha256(b"nothing"))?;
    assert_refused(&rig, missing, "cannot pull snapshot artifact").await?;

    let client = rig.registry.client()?;
    let garbage = push_snapshot_artifact(&client, &rig.registry.address, "garbage", b"not gzip".to_vec())
        .await?
        .0;
    let request = restore_request(&rig.registry.address, "garbage", garbage.digest.as_str())?;
    assert_refused(&rig, request, "cannot decompress").await?;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&[b'x'; 1024])?;
    let broken_tar = push_snapshot_artifact(&client, &rig.registry.address, "broken", encoder.finish()?)
        .await?
        .0;
    let request = restore_request(&rig.registry.address, "broken", broken_tar.digest.as_str())?;
    assert_refused(&rig, request, "cannot unpack").await?;
    Ok(())
}

#[tokio::test]
async fn refuses_an_artifact_with_no_snapshot_layer() -> TestResult {
    let rig = Rig::start().await?;
    let client = rig.registry.client()?;
    let address = &rig.registry.address;
    let kind = ArtifactType::HyperlightSnapshotV1;
    let options = Default::default();
    let target = build_push_target(address, "empty", kind, &[], &options)?;
    client.push_artifact(&target, Vec::new(), Default::default()).await?;
    let request = restore_request(&rig.registry.address, "empty", target.digest.as_str())?;
    assert_refused(&rig, request, "no layers").await?;
    Ok(())
}

#[tokio::test]
async fn reports_a_hypervisor_that_cannot_load_a_layout() -> TestResult {
    let rig = Rig::start().await?;
    rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    let receipt = rig.provider.snapshot(&lease("l1"), &rig.target("snaps")?).await?;
    rig.hypervisor.fail_load(true);
    let request = restore_request(&rig.registry.address, "snaps", receipt.digest.as_str())?;
    assert_refused(&rig, request, "acquisition failed").await?;
    Ok(())
}

#[test]
fn labels_the_snapshot_layer() {
    let layer = snapshot_layer(vec![1, 2]);
    assert_eq!(layer.media_type, HYPERLIGHT_SNAPSHOT_MEDIA_TYPE);
    assert_eq!(layer.data, vec![1, 2]);
}
