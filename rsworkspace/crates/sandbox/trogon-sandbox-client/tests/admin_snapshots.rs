mod admin_support;
mod common;

use std::time::Duration;

use admin_support::{digest, tenant, unauthorized};
use common::{TestResult, client_replying};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    DeleteNamedSandboxResponse, DeleteSnapshotTagResponse, LeaseError, LeaseId, LeaseName, SetSnapshotTagResponse,
    SnapshotGetResponse, SnapshotTagHistoryResponse, SnapshotTagName, SnapshotTreeResponse,
};
use trogon_sandbox_protocol::subjects;

#[tokio::test]
async fn snapshot_get_returns_the_answer_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&SnapshotGetResponse::NotFound)?;
    let got = client.snapshot_get(tenant()?, digest()?).await?;
    assert_eq!(got, SnapshotGetResponse::NotFound);
    assert_eq!(log.only()?.subject, subjects::SNAPSHOT_GET_REQUEST);

    let (client, _) = client_replying(&SnapshotGetResponse::Denied(unauthorized()))?;
    let denied = client.snapshot_get(tenant()?, digest()?).await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

#[tokio::test]
async fn snapshot_tree_returns_the_nodes_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&SnapshotTreeResponse {
        nodes: Vec::new(),
        denied: None,
    })?;
    let tree = client.snapshot_tree(tenant()?, digest()?).await?;
    assert!(tree.nodes.is_empty());
    assert_eq!(log.only()?.subject, subjects::SNAPSHOT_TREE_REQUEST);

    let (client, _) = client_replying(&SnapshotTreeResponse {
        nodes: Vec::new(),
        denied: Some(unauthorized()),
    })?;
    let denied = client.snapshot_tree(tenant()?, digest()?).await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

#[tokio::test]
async fn set_snapshot_tag_applies_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&SetSnapshotTagResponse::Applied)?;
    client
        .set_snapshot_tag(tenant()?, SnapshotTagName::new("main")?, digest()?)
        .await?;
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::SNAPSHOT_TAG_SET_REQUEST);
    assert_eq!(sent.json()?["name"], "main");

    let (client, _) = client_replying(&SetSnapshotTagResponse::Denied(unauthorized()))?;
    let denied = client
        .set_snapshot_tag(tenant()?, SnapshotTagName::new("main")?, digest()?)
        .await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

#[tokio::test]
async fn delete_snapshot_tag_returns_the_answer_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&DeleteSnapshotTagResponse::NotFound)?;
    let got = client
        .delete_snapshot_tag(tenant()?, SnapshotTagName::new("main")?)
        .await?;
    assert_eq!(got, DeleteSnapshotTagResponse::NotFound);
    assert_eq!(log.only()?.subject, subjects::SNAPSHOT_TAG_DELETE_REQUEST);

    let (client, _) = client_replying(&DeleteSnapshotTagResponse::Denied(unauthorized()))?;
    let denied = client
        .delete_snapshot_tag(tenant()?, SnapshotTagName::new("main")?)
        .await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

#[tokio::test]
async fn snapshot_tag_history_returns_entries_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&SnapshotTagHistoryResponse {
        entries: Vec::new(),
        denied: None,
    })?;
    let history = client
        .snapshot_tag_history(tenant()?, SnapshotTagName::new("main")?)
        .await?;
    assert!(history.entries.is_empty());
    assert_eq!(log.only()?.subject, subjects::SNAPSHOT_TAG_HISTORY_REQUEST);

    let (client, _) = client_replying(&SnapshotTagHistoryResponse {
        entries: Vec::new(),
        denied: Some(unauthorized()),
    })?;
    let denied = client
        .snapshot_tag_history(tenant()?, SnapshotTagName::new("main")?)
        .await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

#[tokio::test]
async fn delete_named_sandbox_reports_each_outcome() -> TestResult {
    let (client, log) = client_replying(&DeleteNamedSandboxResponse::Deleted)?;
    let deleted = client.delete_named_sandbox(tenant()?, LeaseName::new("repl-1")?).await;
    assert!(matches!(deleted, Ok(DeleteNamedSandboxResponse::Deleted)));
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::DELETE_NAMED_SANDBOX_REQUEST);
    assert_eq!(sent.timeout, Duration::from_secs(20));

    let (client, _) = client_replying(&DeleteNamedSandboxResponse::NotFound)?;
    let missing = client.delete_named_sandbox(tenant()?, LeaseName::new("repl-1")?).await;
    assert!(matches!(missing, Ok(DeleteNamedSandboxResponse::NotFound)));

    let (client, _) = client_replying(&DeleteNamedSandboxResponse::Denied(LeaseError::LeaseNotFound {
        lease_id: LeaseId::new("lease-1"),
    }))?;
    let denied = client.delete_named_sandbox(tenant()?, LeaseName::new("repl-1")?).await;
    assert!(matches!(
        denied,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    Ok(())
}
