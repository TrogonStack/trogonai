mod common;

use common::{TestResult, client_replying, lease_id, tenant};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    Digest, LeaseError, NativeSnapshotAcceptance, NativeSnapshotDeleteResponse, NativeSnapshotId, NativeSnapshotRef,
    ProviderKind, SnapshotDeleteResponse, SnapshotLeaseResponse,
};

#[tokio::test]
async fn snapshot_lease_maps_capability_answers_and_denials() -> TestResult {
    let (client, log) = client_replying(&SnapshotLeaseResponse::Unsupported)?;
    let out = client.snapshot_lease(tenant()?, &lease_id()).await?;
    assert_eq!(out, SnapshotLeaseResponse::Unsupported);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.snapshot.lease-1");
    assert_eq!(sent.json()?["accept_native"], false);

    let denied = SnapshotLeaseResponse::Denied(LeaseError::LeaseNotFound { lease_id: lease_id() });
    let (client, _) = client_replying(&denied)?;
    let out = client.snapshot_lease(tenant()?, &lease_id()).await;
    assert!(matches!(
        out,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn snapshot_lease_can_accept_a_native_receipt() -> TestResult {
    let (client, log) = client_replying(&SnapshotLeaseResponse::Unsupported)?;
    client
        .snapshot_lease_accepting_native(tenant()?, &lease_id(), NativeSnapshotAcceptance::Accepted)
        .await?;
    assert_eq!(log.only()?.json()?["accept_native"], true);
    Ok(())
}

#[tokio::test]
async fn snapshot_delete_maps_the_answer_and_denial() -> TestResult {
    let digest: Digest = format!("sha256:{}", "c".repeat(64)).parse()?;
    let (client, log) = client_replying(&SnapshotDeleteResponse::Deleted { bytes: 5 })?;
    let out = client.snapshot_delete(tenant()?, digest.clone()).await?;
    assert_eq!(out, SnapshotDeleteResponse::Deleted { bytes: 5 });
    assert_eq!(log.only()?.subject, "sandbox.snapshot.delete");

    let denied = SnapshotDeleteResponse::Denied(LeaseError::Backend { message: "down".into() });
    let (client, _) = client_replying(&denied)?;
    let out = client.snapshot_delete(tenant()?, digest).await;
    assert!(matches!(out, Err(ClientError::Denied(LeaseError::Backend { .. }))));
    Ok(())
}

#[tokio::test]
async fn native_snapshot_delete_maps_provider_answers_and_denial() -> TestResult {
    let reference = NativeSnapshotRef {
        provider: ProviderKind::E2b,
        id: NativeSnapshotId::new("opaque-capture")?,
        expires_at: None,
    };
    for expected in [
        NativeSnapshotDeleteResponse::Deleted,
        NativeSnapshotDeleteResponse::NotFound,
        NativeSnapshotDeleteResponse::Unsupported,
    ] {
        let (client, log) = client_replying(&expected)?;
        let out = client.native_snapshot_delete(tenant()?, reference.clone()).await?;
        assert_eq!(out, expected);
        assert_eq!(log.only()?.subject, "sandbox.native_snapshot.delete");
    }
    let denied = NativeSnapshotDeleteResponse::Denied(LeaseError::Backend {
        message: "cleanup pending".into(),
    });
    let (client, _) = client_replying(&denied)?;
    let out = client.native_snapshot_delete(tenant()?, reference).await;
    assert!(matches!(out, Err(ClientError::Denied(LeaseError::Backend { .. }))));
    Ok(())
}
