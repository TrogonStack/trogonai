use std::time::{Duration, SystemTime};

use serde_json::json;
use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, DeleteSnapshotTagResponse, GetLeaseByNameResponse,
    GetSandboxDomainResponse, LeaseCreation, LeaseGranted, LeaseId, LeaseResponse, ModifyGroupMembershipResponse,
    NativeHandleResponse, ProviderKind, SetSnapshotTagResponse, SnapshotGetResponse, SnapshotTagHistoryResponse,
    SnapshotTreeResponse, SshAccessResponse,
};
use trogon_sandbox_protocol::subjects;

use super::support::call;
use crate::testing::TestResult;

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn granted() -> LeaseGranted {
    LeaseGranted {
        lease_id: LeaseId::new("lease-fork"),
        provider: ProviderKind::Mock,
        sandbox_id: "sandbox-1".into(),
        evidence_subject: "sandbox.evidence.lease-fork.>".into(),
        deadline: SystemTime::now() + Duration::from_secs(300),
        created: LeaseCreation::Created,
        capabilities: Vec::new(),
        entity_id: None,
        operation_id: None,
    }
}

#[tokio::test]
async fn a_fork_sends_the_tenant_and_key() -> TestResult {
    let args = json!({"source_lease_id": "src", "ttl_seconds": 60, "idempotency_key": "k1"});
    let outcome = call("sandbox_fork", args, &LeaseResponse::Granted(granted())).await?;
    assert!(!outcome.is_error());
    let sent = outcome.sent.first().ok_or("nothing was sent")?;
    assert_eq!(sent.subject, subjects::LEASE_FORK_REQUEST);
    let body = sent.json()?;
    assert_eq!(
        (body["tenant"].as_str(), body["idempotency_key"].as_str()),
        (Some("acme"), Some("k1"))
    );
    Ok(())
}

#[tokio::test]
async fn account_tools_reach_the_lease_subjects() -> TestResult {
    let lease = LeaseId::new("l1");
    let user = call(
        "sandbox_create_user",
        json!({"lease_id": "l1", "username": "dev"}),
        &CreateSandboxUserResponse::Created {
            home_dir: "/home/dev".into(),
        },
    )
    .await?;
    assert!(!user.is_error());
    assert_eq!(
        user.sent.first().map(|s| s.subject.clone()),
        Some(subjects::lease_create_user(&lease))
    );

    let group = call(
        "sandbox_create_group",
        json!({"lease_id": "l1", "groupname": "team"}),
        &CreateSandboxGroupResponse::Created {
            shared_dir: "/shared/team".into(),
        },
    )
    .await?;
    assert!(!group.is_error());
    assert_eq!(
        group.sent.first().map(|s| s.subject.clone()),
        Some(subjects::lease_create_group(&lease))
    );

    let member = call(
        "sandbox_modify_group_membership",
        json!({"lease_id": "l1", "username": "dev", "groupname": "team", "action": "add"}),
        &ModifyGroupMembershipResponse::Applied,
    )
    .await?;
    assert!(!member.is_error());
    assert_eq!(
        member
            .sent
            .first()
            .map(|s| s.json())
            .transpose()?
            .map(|b| b["action"].clone()),
        Some(json!("add"))
    );
    Ok(())
}

#[tokio::test]
async fn a_bad_account_input_never_reaches_the_bus() -> TestResult {
    let bad_action = call(
        "sandbox_modify_group_membership",
        json!({"lease_id": "l1", "username": "dev", "groupname": "team", "action": "grant"}),
        &ModifyGroupMembershipResponse::Applied,
    )
    .await?;
    assert!(bad_action.is_error());
    assert!(bad_action.text()?.contains("action"));
    assert!(bad_action.sent.is_empty());

    let bad_user = call(
        "sandbox_create_user",
        json!({"lease_id": "l1", "username": "Not Valid!"}),
        &CreateSandboxUserResponse::Created {
            home_dir: String::new(),
        },
    )
    .await?;
    assert!(bad_user.is_error() && bad_user.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn access_tools_return_the_broker_answer() -> TestResult {
    let domain = call(
        "sandbox_get_domain",
        json!({"lease_id": "l1", "port": 8080}),
        &GetSandboxDomainResponse::PortNotExposed,
    )
    .await?;
    assert!(!domain.is_error());
    assert_eq!(
        domain.result.structured_content,
        Some(json!({"outcome": "port_not_exposed"}))
    );

    let handle = call(
        "sandbox_native_handle",
        json!({"lease_id": "l1"}),
        &NativeHandleResponse::Unsupported,
    )
    .await?;
    assert_eq!(
        handle.result.structured_content,
        Some(json!({"outcome": "unsupported"}))
    );

    let ssh = call(
        "sandbox_ssh_access",
        json!({"lease_id": "l1", "ttl_seconds": 30}),
        &SshAccessResponse::Unsupported,
    )
    .await?;
    assert_eq!(ssh.result.structured_content, Some(json!({"outcome": "unsupported"})));
    assert_eq!(
        ssh.sent
            .first()
            .map(|s| s.json())
            .transpose()?
            .map(|b| b["ttl_seconds"].clone()),
        Some(json!(30))
    );
    Ok(())
}

#[tokio::test]
async fn a_lease_is_found_by_name_within_the_tenant() -> TestResult {
    let outcome = call(
        "sandbox_get_lease_by_name",
        json!({"name": "dev-box"}),
        &GetLeaseByNameResponse::NotFound,
    )
    .await?;
    assert_eq!(outcome.result.structured_content, Some(json!({"outcome": "not_found"})));
    let body = outcome.sent.first().ok_or("nothing was sent")?.json()?;
    assert_eq!(
        (body["tenant"].as_str(), body["name"].as_str()),
        (Some("acme"), Some("dev-box"))
    );
    assert!(
        call(
            "sandbox_get_lease_by_name",
            json!({"name": ""}),
            &GetLeaseByNameResponse::NotFound
        )
        .await?
        .is_error()
    );
    Ok(())
}

#[tokio::test]
async fn snapshot_tools_are_scoped_to_the_tenant() -> TestResult {
    let got = call(
        "sandbox_snapshot_get",
        json!({"digest": DIGEST}),
        &SnapshotGetResponse::NotFound,
    )
    .await?;
    assert_eq!(got.result.structured_content, Some(json!({"outcome": "not_found"})));
    assert_eq!(
        got.sent
            .first()
            .map(|s| s.json())
            .transpose()?
            .map(|b| b["tenant"].clone()),
        Some(json!("acme"))
    );

    let tree = call(
        "sandbox_snapshot_tree",
        json!({"digest": DIGEST}),
        &SnapshotTreeResponse {
            nodes: Vec::new(),
            denied: None,
        },
    )
    .await?;
    assert!(!tree.is_error());

    let tagged = call(
        "sandbox_set_snapshot_tag",
        json!({"name": "stable", "digest": DIGEST}),
        &SetSnapshotTagResponse::Applied,
    )
    .await?;
    assert_eq!(tagged.result.structured_content, Some(json!({"ack": true})));

    let deleted = call(
        "sandbox_delete_snapshot_tag",
        json!({"name": "stable"}),
        &DeleteSnapshotTagResponse::Applied,
    )
    .await?;
    assert_eq!(deleted.result.structured_content, Some(json!({"outcome": "applied"})));

    let history = call(
        "sandbox_snapshot_tag_history",
        json!({"name": "stable"}),
        &SnapshotTagHistoryResponse {
            entries: Vec::new(),
            denied: None,
        },
    )
    .await?;
    assert!(!history.is_error());
    Ok(())
}

#[tokio::test]
async fn a_malformed_digest_or_tag_never_reaches_the_bus() -> TestResult {
    for (tool, args) in [
        ("sandbox_snapshot_get", json!({"digest": "nope"})),
        ("sandbox_snapshot_tree", json!({"digest": "nope"})),
        ("sandbox_set_snapshot_tag", json!({"name": "stable", "digest": "nope"})),
        ("sandbox_set_snapshot_tag", json!({"name": "", "digest": DIGEST})),
        ("sandbox_delete_snapshot_tag", json!({"name": ""})),
        ("sandbox_snapshot_tag_history", json!({"name": ""})),
    ] {
        let outcome = call(tool, args, &SetSnapshotTagResponse::Applied).await?;
        assert!(outcome.is_error() && outcome.sent.is_empty(), "{tool}");
    }
    Ok(())
}
