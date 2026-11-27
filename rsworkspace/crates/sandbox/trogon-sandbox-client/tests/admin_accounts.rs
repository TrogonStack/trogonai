mod admin_support;
mod common;

use std::time::Duration;

use common::{MockBus, TestResult, client_replying, lease_id};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, GroupMembershipAction, LeaseError,
    ModifyGroupMembershipResponse, SandboxGroupName, SandboxUserName,
};
use trogon_sandbox_protocol::subjects;

#[tokio::test]
async fn create_user_sends_the_request_and_returns_the_outcome() -> TestResult {
    let reply = CreateSandboxUserResponse::Created {
        home_dir: "/home/alice".into(),
    };
    let (client, log) = client_replying(&reply)?;

    let response = client.create_user(&lease_id(), SandboxUserName::new("alice")?).await?;

    assert_eq!(response, reply);
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::lease_create_user(&lease_id()));
    assert_eq!(sent.timeout, Duration::from_secs(30));
    assert_eq!(sent.json()?["username"], "alice");
    Ok(())
}

#[tokio::test]
async fn create_user_surfaces_denial_and_timeout() -> TestResult {
    let (client, _) = client_replying(&CreateSandboxUserResponse::Denied(admin_support::unauthorized()))?;
    let denied = client.create_user(&lease_id(), SandboxUserName::new("alice")?).await;
    assert!(matches!(
        denied,
        Err(ClientError::Denied(LeaseError::Unauthorized { .. }))
    ));

    let timed_out = MockBus::timing_out()
        .into_client()
        .create_user(&lease_id(), SandboxUserName::new("alice")?)
        .await;
    assert!(matches!(timed_out, Err(ClientError::Timeout(_))));
    Ok(())
}

#[tokio::test]
async fn create_group_sends_the_request_and_returns_the_outcome() -> TestResult {
    let reply = CreateSandboxGroupResponse::AlreadyExists {
        shared_dir: "/srv/devs".into(),
    };
    let (client, log) = client_replying(&reply)?;

    let response = client.create_group(&lease_id(), SandboxGroupName::new("devs")?).await?;

    assert_eq!(response, reply);
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::lease_create_group(&lease_id()));
    assert_eq!(sent.timeout, Duration::from_secs(30));
    assert_eq!(sent.json()?["groupname"], "devs");
    Ok(())
}

#[tokio::test]
async fn create_group_surfaces_denial() -> TestResult {
    let (client, _) = client_replying(&CreateSandboxGroupResponse::Denied(admin_support::unauthorized()))?;
    let denied = client.create_group(&lease_id(), SandboxGroupName::new("devs")?).await;
    assert!(matches!(denied, Err(ClientError::Denied(_))));
    Ok(())
}

#[tokio::test]
async fn modify_group_membership_applies_and_sends_the_action() -> TestResult {
    let (client, log) = client_replying(&ModifyGroupMembershipResponse::Applied)?;

    client
        .modify_group_membership(
            &lease_id(),
            SandboxUserName::new("alice")?,
            SandboxGroupName::new("devs")?,
            GroupMembershipAction::Add,
        )
        .await?;

    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::lease_modify_group_membership(&lease_id()));
    assert_eq!(sent.timeout, Duration::from_secs(30));
    assert_eq!(sent.json()?["action"], "add");
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the rendered message is the contract under test; typed variants are matched separately"
    )
)]
async fn modify_group_membership_maps_missing_dependency_and_denial() -> TestResult {
    let (client, _) = client_replying(&ModifyGroupMembershipResponse::MissingDependency { tool: "gpasswd".into() })?;
    let missing = client
        .modify_group_membership(
            &lease_id(),
            SandboxUserName::new("alice")?,
            SandboxGroupName::new("devs")?,
            GroupMembershipAction::Remove,
        )
        .await;
    let Err(error) = missing else {
        return Err("expected a transport error".into());
    };
    assert!(matches!(error, ClientError::Transport(_)));
    assert!(error.to_string().contains("gpasswd"), "{error}");

    let (client, _) = client_replying(&ModifyGroupMembershipResponse::Denied(admin_support::unauthorized()))?;
    let denied = client
        .modify_group_membership(
            &lease_id(),
            SandboxUserName::new("alice")?,
            SandboxGroupName::new("devs")?,
            GroupMembershipAction::Add,
        )
        .await;
    assert!(matches!(denied, Err(ClientError::Denied(_))));
    Ok(())
}
