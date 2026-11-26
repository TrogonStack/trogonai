use std::str::FromStr;

use serde_json::json;
use trogon_sandbox_contract::{
    CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse,
    GroupMembershipAction, LeaseError, LeaseId, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse,
    SandboxGroupName, SandboxUserName, UnixNameError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn unix_names_accept_the_useradd_grammar() -> TestResult {
    let longest = "a".repeat(32);
    for value in ["root", "_svc", "agent-a", "a1_b-2", longest.as_str()] {
        let user = SandboxUserName::new(value)?;
        assert_eq!(user.as_str(), value);
        assert_eq!(user.to_string(), value);
        assert_eq!(SandboxUserName::from_str(value)?, user);
        assert_eq!(SandboxUserName::try_from(value.to_owned())?, user);
        assert_eq!(serde_json::to_value(&user)?, json!(value));
        assert_eq!(serde_json::from_value::<SandboxUserName>(json!(value))?, user);
        assert_eq!(String::from(user), value);

        let group = SandboxGroupName::new(value)?;
        assert_eq!(group.as_str(), value);
        assert_eq!(group.to_string(), value);
        assert_eq!(SandboxGroupName::from_str(value)?, group);
        assert_eq!(SandboxGroupName::try_from(value.to_owned())?, group);
        assert_eq!(serde_json::to_value(&group)?, json!(value));
        assert_eq!(serde_json::from_value::<SandboxGroupName>(json!(value))?, group);
        assert_eq!(String::from(group), value);
    }
    Ok(())
}

#[test]
fn unix_names_reject_everything_else() {
    let too_long = "a".repeat(33);
    for (value, expected) in [
        ("", UnixNameError::InvalidLength),
        (too_long.as_str(), UnixNameError::InvalidLength),
        ("Root", UnixNameError::InvalidCharacters),
        ("1abc", UnixNameError::InvalidCharacters),
        ("-abc", UnixNameError::InvalidCharacters),
        ("agent.a", UnixNameError::InvalidCharacters),
        ("agent a", UnixNameError::InvalidCharacters),
        ("a;rm", UnixNameError::InvalidCharacters),
        ("agentA", UnixNameError::InvalidCharacters),
        ("\u{e9}t\u{e9}", UnixNameError::InvalidCharacters),
        ("a\u{e9}", UnixNameError::InvalidCharacters),
    ] {
        assert_eq!(SandboxUserName::new(value).err(), Some(expected), "{value:?}");
        assert_eq!(SandboxGroupName::new(value).err(), Some(expected), "{value:?}");

        let user_error = serde_json::from_value::<SandboxUserName>(json!(value)).err();
        assert_eq!(user_error.map(|error| error.to_string()), Some(expected.to_string()));
        let group_error = serde_json::from_value::<SandboxGroupName>(json!(value)).err();
        assert_eq!(group_error.map(|error| error.to_string()), Some(expected.to_string()));
    }
    assert_eq!(UnixNameError::InvalidLength.to_string(), "name must be 1-32 characters");
    assert_eq!(
        UnixNameError::InvalidCharacters.to_string(),
        "name must start with [a-z_] and contain only [a-z0-9_-]"
    );
}

#[test]
fn account_requests_carry_validated_names() -> TestResult {
    let user = CreateSandboxUserRequest {
        lease_id: LeaseId::new("lease-1"),
        username: SandboxUserName::new("agent-a")?,
    };
    let wire = json!({ "lease_id": "lease-1", "username": "agent-a" });
    assert_eq!(serde_json::to_value(&user)?, wire);
    assert_eq!(serde_json::from_value::<CreateSandboxUserRequest>(wire)?, user);
    let invalid = json!({ "lease_id": "lease-1", "username": "Agent" });
    assert!(serde_json::from_value::<CreateSandboxUserRequest>(invalid).is_err());

    let group = CreateSandboxGroupRequest {
        lease_id: LeaseId::new("lease-1"),
        groupname: SandboxGroupName::new("team")?,
    };
    let wire = json!({ "lease_id": "lease-1", "groupname": "team" });
    assert_eq!(serde_json::to_value(&group)?, wire);
    assert_eq!(serde_json::from_value::<CreateSandboxGroupRequest>(wire)?, group);
    let invalid = json!({ "lease_id": "lease-1", "groupname": "1team" });
    assert!(serde_json::from_value::<CreateSandboxGroupRequest>(invalid).is_err());

    for (action, name) in [
        (GroupMembershipAction::Add, "add"),
        (GroupMembershipAction::Remove, "remove"),
    ] {
        let request = ModifyGroupMembershipRequest {
            lease_id: LeaseId::new("lease-1"),
            username: SandboxUserName::new("agent-a")?,
            groupname: SandboxGroupName::new("team")?,
            action,
        };
        let wire = json!({ "lease_id": "lease-1", "username": "agent-a", "groupname": "team", "action": name });
        assert_eq!(serde_json::to_value(&request)?, wire);
        assert_eq!(serde_json::from_value::<ModifyGroupMembershipRequest>(wire)?, request);
    }
    let invalid = json!({ "lease_id": "lease-1", "username": "agent-a", "groupname": "team", "action": "toggle" });
    assert!(serde_json::from_value::<ModifyGroupMembershipRequest>(invalid).is_err());
    Ok(())
}

#[test]
fn account_responses_are_tagged_by_outcome() -> TestResult {
    let denied_wire = json!({ "outcome": "denied", "kind": "no_provider_available" });

    for (response, wire) in [
        (
            CreateSandboxUserResponse::Created {
                home_dir: "/home/agent-a".to_owned(),
            },
            json!({ "outcome": "created", "home_dir": "/home/agent-a" }),
        ),
        (
            CreateSandboxUserResponse::AlreadyExists {
                home_dir: "/home/agent-a".to_owned(),
            },
            json!({ "outcome": "already_exists", "home_dir": "/home/agent-a" }),
        ),
        (
            CreateSandboxUserResponse::MissingDependency {
                tool: "useradd".to_owned(),
            },
            json!({ "outcome": "missing_dependency", "tool": "useradd" }),
        ),
        (
            CreateSandboxUserResponse::Denied(LeaseError::NoProviderAvailable),
            denied_wire.clone(),
        ),
    ] {
        assert_eq!(serde_json::to_value(&response)?, wire);
        assert_eq!(serde_json::from_value::<CreateSandboxUserResponse>(wire)?, response);
    }

    for (response, wire) in [
        (
            CreateSandboxGroupResponse::Created {
                shared_dir: "/shared/team".to_owned(),
            },
            json!({ "outcome": "created", "shared_dir": "/shared/team" }),
        ),
        (
            CreateSandboxGroupResponse::AlreadyExists {
                shared_dir: "/shared/team".to_owned(),
            },
            json!({ "outcome": "already_exists", "shared_dir": "/shared/team" }),
        ),
        (
            CreateSandboxGroupResponse::MissingDependency {
                tool: "groupadd".to_owned(),
            },
            json!({ "outcome": "missing_dependency", "tool": "groupadd" }),
        ),
        (
            CreateSandboxGroupResponse::Denied(LeaseError::NoProviderAvailable),
            denied_wire.clone(),
        ),
    ] {
        assert_eq!(serde_json::to_value(&response)?, wire);
        assert_eq!(serde_json::from_value::<CreateSandboxGroupResponse>(wire)?, response);
    }

    for (response, wire) in [
        (ModifyGroupMembershipResponse::Applied, json!({ "outcome": "applied" })),
        (
            ModifyGroupMembershipResponse::MissingDependency {
                tool: "usermod".to_owned(),
            },
            json!({ "outcome": "missing_dependency", "tool": "usermod" }),
        ),
        (
            ModifyGroupMembershipResponse::Denied(LeaseError::NoProviderAvailable),
            denied_wire,
        ),
    ] {
        assert_eq!(serde_json::to_value(&response)?, wire);
        assert_eq!(serde_json::from_value::<ModifyGroupMembershipResponse>(wire)?, response);
    }
    Ok(())
}
