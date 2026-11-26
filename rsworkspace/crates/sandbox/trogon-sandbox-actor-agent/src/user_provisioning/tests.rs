use std::error::Error;
use std::sync::Mutex;

use trogon_sandbox_contract::{LeaseId, SandboxGroupName, SandboxUserName};

use super::*;

/// Answers each script by the first rule whose needle it contains; an unmatched script succeeds.
struct Scripted {
    rules: Vec<(&'static str, Result<ScriptOutcome, LeaseError>)>,
    seen: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(rules: Vec<(&'static str, Result<ScriptOutcome, LeaseError>)>) -> Self {
        Self {
            rules,
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl ScriptRunner for Scripted {
    async fn run(&self, _shell: &str, script: &str) -> Result<ScriptOutcome, LeaseError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(script.to_string());
        }
        for (needle, answer) in &self.rules {
            if script.contains(needle) {
                return answer.clone();
            }
        }
        Ok(ScriptOutcome::Succeeded)
    }
}

fn failed(code: i32, stderr: &str) -> Result<ScriptOutcome, LeaseError> {
    Ok(ScriptOutcome::Failed {
        code,
        stderr: stderr.to_string(),
    })
}

fn broken() -> Result<ScriptOutcome, LeaseError> {
    Err(LeaseError::Backend {
        message: "no shell".to_string(),
    })
}

fn user_request() -> Result<CreateSandboxUserRequest, Box<dyn Error>> {
    Ok(CreateSandboxUserRequest {
        lease_id: LeaseId::new("lease"),
        username: SandboxUserName::new("alice")?,
    })
}

fn group_request() -> Result<CreateSandboxGroupRequest, Box<dyn Error>> {
    Ok(CreateSandboxGroupRequest {
        lease_id: LeaseId::new("lease"),
        groupname: SandboxGroupName::new("agents")?,
    })
}

fn membership(action: GroupMembershipAction) -> Result<ModifyGroupMembershipRequest, Box<dyn Error>> {
    Ok(ModifyGroupMembershipRequest {
        lease_id: LeaseId::new("lease"),
        username: SandboxUserName::new("alice")?,
        groupname: SandboxGroupName::new("agents")?,
        action,
    })
}

#[tokio::test]
async fn a_new_user_is_created_with_a_private_home() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("id -u", failed(1, ""))]);
    let response = create_user_with(&runner, &user_request()?).await;
    assert!(
        matches!(response, CreateSandboxUserResponse::Created { .. }),
        "{response:?}"
    );
    let seen = runner.seen.lock().map_err(|error| error.to_string())?;
    assert!(
        seen.iter()
            .any(|script| script.contains("useradd -m") && script.contains("chmod 770"))
    );
    Ok(())
}

#[tokio::test]
async fn an_existing_user_is_reported_without_running_useradd() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![]);
    let response = create_user_with(&runner, &user_request()?).await;
    assert!(
        matches!(response, CreateSandboxUserResponse::AlreadyExists { .. }),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_useradd_is_a_dependency_answer() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("'useradd'", failed(1, ""))]);
    let response = create_user_with(&runner, &user_request()?).await;
    assert_eq!(
        response,
        CreateSandboxUserResponse::MissingDependency {
            tool: "useradd".to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn an_unstartable_shell_is_named_as_the_missing_tool() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("command -v", broken())]);
    let response = create_user_with(&runner, &user_request()?).await;
    assert_eq!(
        response,
        CreateSandboxUserResponse::MissingDependency {
            tool: SHELL.to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_shell_after_the_first_tool_is_still_named_as_the_shell() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("'/bin/bash'", failed(1, ""))]);
    let response = create_user_with(&runner, &user_request()?).await;
    assert_eq!(
        response,
        CreateSandboxUserResponse::MissingDependency {
            tool: SHELL.to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_user_probe_that_cannot_run_is_denied() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("id -u", broken())]);
    let response = create_user_with(&runner, &user_request()?).await;
    assert!(matches!(response, CreateSandboxUserResponse::Denied(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn a_user_creation_that_cannot_run_is_denied() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("id -u", failed(1, "")), ("useradd -m", broken())]);
    let response = create_user_with(&runner, &user_request()?).await;
    assert!(matches!(response, CreateSandboxUserResponse::Denied(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn a_failed_user_creation_is_denied_with_the_reason() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![
        ("id -u", failed(1, "")),
        ("useradd -m", failed(1, "Permission denied")),
    ]);
    let response = create_user_with(&runner, &user_request()?).await;
    let CreateSandboxUserResponse::Denied(LeaseError::Backend { message }) = response else {
        return Err("expected a denied backend answer".into());
    };
    assert!(message.contains("not running as root"), "{message}");
    Ok(())
}

#[tokio::test]
async fn a_new_group_gets_a_setgid_shared_directory() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("getent", failed(2, ""))]);
    let response = create_group_with(&runner, &group_request()?).await;
    assert!(
        matches!(response, CreateSandboxGroupResponse::Created { .. }),
        "{response:?}"
    );
    let seen = runner.seen.lock().map_err(|error| error.to_string())?;
    assert!(seen.iter().any(|script| script.contains("chmod 2770")));
    Ok(())
}

#[tokio::test]
async fn an_existing_group_is_reported() -> Result<(), Box<dyn Error>> {
    let response = create_group_with(&Scripted::new(vec![]), &group_request()?).await;
    assert!(
        matches!(response, CreateSandboxGroupResponse::AlreadyExists { .. }),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_groupadd_is_a_dependency_answer() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("'groupadd'", failed(1, ""))]);
    let response = create_group_with(&runner, &group_request()?).await;
    assert_eq!(
        response,
        CreateSandboxGroupResponse::MissingDependency {
            tool: "groupadd".to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_group_probe_that_cannot_run_is_denied() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("getent", broken())]);
    let response = create_group_with(&runner, &group_request()?).await;
    assert!(
        matches!(response, CreateSandboxGroupResponse::Denied(_)),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_group_creation_that_cannot_run_is_denied() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("getent", failed(2, "")), ("groupadd '", broken())]);
    let response = create_group_with(&runner, &group_request()?).await;
    assert!(
        matches!(response, CreateSandboxGroupResponse::Denied(_)),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_group_creation_is_denied_with_the_reason() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("getent", failed(2, "")), ("groupadd '", failed(9, "taken"))]);
    let response = create_group_with(&runner, &group_request()?).await;
    let CreateSandboxGroupResponse::Denied(LeaseError::Backend { message }) = response else {
        return Err("expected a denied backend answer".into());
    };
    assert!(message.contains("exit 9"), "{message}");
    Ok(())
}

#[tokio::test]
async fn adding_and_removing_use_different_tools() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![]);
    let added = modify_group_membership_with(&runner, &membership(GroupMembershipAction::Add)?).await;
    let removed = modify_group_membership_with(&runner, &membership(GroupMembershipAction::Remove)?).await;
    assert_eq!(added, ModifyGroupMembershipResponse::Applied);
    assert_eq!(removed, ModifyGroupMembershipResponse::Applied);
    let seen = runner.seen.lock().map_err(|error| error.to_string())?;
    assert!(seen.iter().any(|script| script.contains("usermod -aG")));
    assert!(seen.iter().any(|script| script.contains("gpasswd -d")));
    Ok(())
}

#[tokio::test]
async fn a_missing_membership_tool_is_a_dependency_answer() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("'gpasswd'", failed(1, ""))]);
    let response = modify_group_membership_with(&runner, &membership(GroupMembershipAction::Remove)?).await;
    assert_eq!(
        response,
        ModifyGroupMembershipResponse::MissingDependency {
            tool: "gpasswd".to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn membership_failures_are_denied() -> Result<(), Box<dyn Error>> {
    let runner = Scripted::new(vec![("usermod -aG", broken())]);
    let response = modify_group_membership_with(&runner, &membership(GroupMembershipAction::Add)?).await;
    assert!(
        matches!(response, ModifyGroupMembershipResponse::Denied(_)),
        "{response:?}"
    );

    let runner = Scripted::new(vec![("usermod -aG", failed(6, "no such group"))]);
    let response = modify_group_membership_with(&runner, &membership(GroupMembershipAction::Add)?).await;
    assert!(
        matches!(response, ModifyGroupMembershipResponse::Denied(_)),
        "{response:?}"
    );
    Ok(())
}

#[test]
fn a_permission_error_is_reported_as_the_agent_not_being_root() {
    for stderr in ["useradd: Permission denied.", "must be run as root"] {
        let error = provisioning_failed("useradd", 1, stderr);
        assert!(matches!(&error, LeaseError::Backend { message } if message.contains("not running as root")));
    }
}

#[test]
fn an_ordinary_failure_keeps_the_exit_code_and_stderr() {
    let error = provisioning_failed("groupadd", 9, "group 'agents' already exists");
    assert!(
        matches!(&error, LeaseError::Backend { message } if message.contains("exit 9") && message.contains("already exists"))
    );
}

#[tokio::test]
async fn the_real_wrappers_answer_without_creating_anything_unprivileged() -> Result<(), Box<dyn Error>> {
    let created = create_user(&user_request()?).await;
    let group = create_group(&group_request()?).await;
    let membership = modify_group_membership(&membership(GroupMembershipAction::Add)?).await;
    let root = nix::unistd::Uid::effective().is_root();
    assert!(root || !matches!(created, CreateSandboxUserResponse::Created { .. }));
    assert!(root || !matches!(group, CreateSandboxGroupResponse::Created { .. }));
    assert!(root || membership != ModifyGroupMembershipResponse::Applied);
    Ok(())
}
