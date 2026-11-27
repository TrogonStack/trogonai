mod support;

use support::*;
use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, GroupMembershipAction, ModifyGroupMembershipResponse,
    SandboxGroupName, SandboxUserName,
};
use trogon_sandbox_protocol::Provider;

#[tokio::test]
async fn user_creation_reports_every_outcome() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-users");
    let user = SandboxUserName::new("alice")?;
    assert!(world.provider.create_user_for_lease(&lease, &user).await.is_none());
    world.provider.lease(&lease_request()?, &lease).await?;

    world.cluster.script(ExecScript::new("id -u").exit(1));
    let created = world
        .provider
        .create_user_for_lease(&lease, &user)
        .await
        .ok_or("no lease")??;
    assert!(matches!(created, CreateSandboxUserResponse::Created { .. }));

    world.cluster.state().scripts.clear();
    let existing = world
        .provider
        .create_user_for_lease(&lease, &user)
        .await
        .ok_or("no lease")??;
    assert!(matches!(existing, CreateSandboxUserResponse::AlreadyExists { .. }));

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("id -u").exit(1));
    world
        .cluster
        .script(ExecScript::new("useradd -m").exit(9).stderr(b"nope"));
    let denied = world
        .provider
        .create_user_for_lease(&lease, &user)
        .await
        .ok_or("no lease")??;
    assert!(matches!(denied, CreateSandboxUserResponse::Denied(_)));

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("command -v /bin/bash").exit(1));
    let no_bash = world
        .provider
        .create_user_for_lease(&lease, &user)
        .await
        .ok_or("no lease")??;
    assert!(matches!(no_bash, CreateSandboxUserResponse::MissingDependency { tool } if tool == "/bin/bash"));

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("command -v useradd").exit(1));
    let no_useradd = world
        .provider
        .create_user_for_lease(&lease, &user)
        .await
        .ok_or("no lease")??;
    assert!(matches!(no_useradd, CreateSandboxUserResponse::MissingDependency { tool } if tool == "useradd"));

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("").refuse(503));
    assert!(
        world
            .provider
            .create_user_for_lease(&lease, &user)
            .await
            .ok_or("no lease")?
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn group_creation_reports_every_outcome() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-groups");
    let group = SandboxGroupName::new("devs")?;
    assert!(world.provider.create_group_for_lease(&lease, &group).await.is_none());
    world.provider.lease(&lease_request()?, &lease).await?;

    world.cluster.script(ExecScript::new("getent group").exit(2));
    let created = world
        .provider
        .create_group_for_lease(&lease, &group)
        .await
        .ok_or("no lease")??;
    assert!(matches!(created, CreateSandboxGroupResponse::Created { .. }));

    world.cluster.state().scripts.clear();
    let existing = world
        .provider
        .create_group_for_lease(&lease, &group)
        .await
        .ok_or("no lease")??;
    assert!(matches!(existing, CreateSandboxGroupResponse::AlreadyExists { .. }));

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("getent group").exit(2));
    world
        .cluster
        .script(ExecScript::new("groupadd '").exit(9).stderr(b"nope"));
    let denied = world
        .provider
        .create_group_for_lease(&lease, &group)
        .await
        .ok_or("no lease")??;
    assert!(matches!(denied, CreateSandboxGroupResponse::Denied(_)));

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("command -v /bin/bash").exit(1));
    let no_bash = world
        .provider
        .create_group_for_lease(&lease, &group)
        .await
        .ok_or("no lease")??;
    assert!(matches!(no_bash, CreateSandboxGroupResponse::MissingDependency { tool } if tool == "/bin/bash"));

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("command -v groupadd").exit(1));
    let no_groupadd = world
        .provider
        .create_group_for_lease(&lease, &group)
        .await
        .ok_or("no lease")??;
    assert!(matches!(no_groupadd, CreateSandboxGroupResponse::MissingDependency { tool } if tool == "groupadd"));
    Ok(())
}

#[tokio::test]
async fn membership_changes_report_every_outcome() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-members");
    let user = SandboxUserName::new("alice")?;
    let group = SandboxGroupName::new("devs")?;
    let add = GroupMembershipAction::Add;
    let remove = GroupMembershipAction::Remove;
    assert!(
        world
            .provider
            .modify_group_membership_for_lease(&lease, &user, &group, add)
            .await
            .is_none()
    );
    world.provider.lease(&lease_request()?, &lease).await?;

    for action in [add, remove] {
        let applied = world
            .provider
            .modify_group_membership_for_lease(&lease, &user, &group, action)
            .await
            .ok_or("no lease")??;
        assert!(matches!(applied, ModifyGroupMembershipResponse::Applied));
    }

    world
        .cluster
        .script(ExecScript::new("usermod -aG").exit(3).stderr(b"nope"));
    let denied = world
        .provider
        .modify_group_membership_for_lease(&lease, &user, &group, add)
        .await
        .ok_or("no lease")??;
    assert!(matches!(denied, ModifyGroupMembershipResponse::Denied(_)));

    world.cluster.state().scripts.clear();
    for (tool, action) in [("usermod", add), ("gpasswd", remove)] {
        world.cluster.state().scripts.clear();
        world
            .cluster
            .script(ExecScript::new(&format!("command -v {tool}")).exit(1));
        let missing = world
            .provider
            .modify_group_membership_for_lease(&lease, &user, &group, action)
            .await
            .ok_or("no lease")??;
        assert!(matches!(missing, ModifyGroupMembershipResponse::MissingDependency { tool: found } if found == tool));
    }
    Ok(())
}
