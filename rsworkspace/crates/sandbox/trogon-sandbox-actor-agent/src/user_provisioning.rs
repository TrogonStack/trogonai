//! Sandbox users and groups, created inside the actor.
//!
//! The same `useradd`/`groupadd`/`usermod` calls other providers make by exec'ing into a sandbox run here, the only
//! place inside the actor that can run them.
//!
//! The agent probes for the tools first, because an image without `useradd` is a normal case that the wire contract
//! models as `MissingDependency`; reporting it as a backend failure would tell a tenant their cluster is broken when
//! their image is simply slim. And the agent has to be root for any of this to work; when it is not, the failure says
//! so rather than surfacing a bare exit code.

use trogon_sandbox_contract::{
    CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse,
    GroupMembershipAction, LeaseError, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse,
};
use trogon_sandbox_protocol::{home_dir_for_user, shared_dir_for_group};

use crate::script_outcome::ScriptOutcome;
use crate::script_runner::{ProcessScriptRunner, ScriptRunner};
use crate::shell_quote::shell_quote;

/// The shell every provisioning command runs through, probed like any other tool rather than assumed.
const SHELL: &str = "/bin/bash";

pub async fn create_user(request: &CreateSandboxUserRequest) -> CreateSandboxUserResponse {
    create_user_with(&ProcessScriptRunner, request).await
}

pub async fn create_group(request: &CreateSandboxGroupRequest) -> CreateSandboxGroupResponse {
    create_group_with(&ProcessScriptRunner, request).await
}

pub async fn modify_group_membership(request: &ModifyGroupMembershipRequest) -> ModifyGroupMembershipResponse {
    modify_group_membership_with(&ProcessScriptRunner, request).await
}

async fn create_user_with(runner: &impl ScriptRunner, request: &CreateSandboxUserRequest) -> CreateSandboxUserResponse {
    let home_dir = home_dir_for_user(&request.username);

    if let Some(tool) = missing_dependency(runner, &["useradd", SHELL]).await {
        return CreateSandboxUserResponse::MissingDependency { tool };
    }
    match exists(runner, &format!("id -u {}", shell_quote(request.username.as_str()))).await {
        Err(error) => return CreateSandboxUserResponse::Denied(error),
        Ok(true) => return CreateSandboxUserResponse::AlreadyExists { home_dir },
        Ok(false) => {}
    }

    // `chmod 770` so a group member can enter the home directory once added, which is what makes a shared group useful
    // at all; the default 755 would let every user read it.
    let script = format!(
        "useradd -m -s {SHELL} {name} && chmod 770 {home}",
        name = shell_quote(request.username.as_str()),
        home = shell_quote(&home_dir),
    );
    match runner.run(SHELL, &script).await {
        Err(error) => CreateSandboxUserResponse::Denied(error),
        Ok(ScriptOutcome::Succeeded) => CreateSandboxUserResponse::Created { home_dir },
        Ok(ScriptOutcome::Failed { code, stderr }) => {
            CreateSandboxUserResponse::Denied(provisioning_failed("useradd", code, &stderr))
        }
    }
}

async fn create_group_with(
    runner: &impl ScriptRunner,
    request: &CreateSandboxGroupRequest,
) -> CreateSandboxGroupResponse {
    let shared_dir = shared_dir_for_group(&request.groupname);

    if let Some(tool) = missing_dependency(runner, &["groupadd", SHELL]).await {
        return CreateSandboxGroupResponse::MissingDependency { tool };
    }
    match exists(
        runner,
        &format!("getent group {}", shell_quote(request.groupname.as_str())),
    )
    .await
    {
        Err(error) => return CreateSandboxGroupResponse::Denied(error),
        Ok(true) => return CreateSandboxGroupResponse::AlreadyExists { shared_dir },
        Ok(false) => {}
    }

    // `2770` is setgid plus group-rwx: files created in here inherit the group instead of the writer's primary group,
    // which is the only reason a caller asks for a shared directory.
    let script = format!(
        "groupadd {name} && mkdir -p {dir} && chgrp {name} {dir} && chmod 2770 {dir}",
        name = shell_quote(request.groupname.as_str()),
        dir = shell_quote(&shared_dir),
    );
    match runner.run(SHELL, &script).await {
        Err(error) => CreateSandboxGroupResponse::Denied(error),
        Ok(ScriptOutcome::Succeeded) => CreateSandboxGroupResponse::Created { shared_dir },
        Ok(ScriptOutcome::Failed { code, stderr }) => {
            CreateSandboxGroupResponse::Denied(provisioning_failed("groupadd", code, &stderr))
        }
    }
}

async fn modify_group_membership_with(
    runner: &impl ScriptRunner,
    request: &ModifyGroupMembershipRequest,
) -> ModifyGroupMembershipResponse {
    // Add and remove need different tools, so an image carrying only one of them is a dependency answer for the action
    // asked for rather than a blanket refusal of both.
    let (tool, script) = match request.action {
        GroupMembershipAction::Add => (
            "usermod",
            format!(
                "usermod -aG {group} {user}",
                group = shell_quote(request.groupname.as_str()),
                user = shell_quote(request.username.as_str()),
            ),
        ),
        GroupMembershipAction::Remove => (
            "gpasswd",
            format!(
                "gpasswd -d {user} {group}",
                user = shell_quote(request.username.as_str()),
                group = shell_quote(request.groupname.as_str()),
            ),
        ),
    };
    if let Some(tool) = missing_dependency(runner, &[tool, SHELL]).await {
        return ModifyGroupMembershipResponse::MissingDependency { tool };
    }

    match runner.run(SHELL, &script).await {
        Err(error) => ModifyGroupMembershipResponse::Denied(error),
        Ok(ScriptOutcome::Succeeded) => ModifyGroupMembershipResponse::Applied,
        Ok(ScriptOutcome::Failed { code, stderr }) => {
            ModifyGroupMembershipResponse::Denied(provisioning_failed(tool, code, &stderr))
        }
    }
}

/// Returns the first tool of `tools` the image does not have.
///
/// `command -v` rather than a path probe so a tool found through `PATH` counts wherever the image put it.
async fn missing_dependency(runner: &impl ScriptRunner, tools: &[&str]) -> Option<String> {
    for tool in tools {
        let script = format!("command -v {} >/dev/null 2>&1", shell_quote(tool));
        match runner.run(SHELL, &script).await {
            Ok(ScriptOutcome::Succeeded) => {}
            // A shell that cannot be started is itself the missing dependency, and probing the rest of the list
            // through it would report the wrong tool.
            Err(_) => return Some(SHELL.to_string()),
            Ok(ScriptOutcome::Failed { .. }) => return Some((*tool).to_string()),
        }
    }
    None
}

/// A non-zero exit means "absent", not "broken".
///
/// `id` and `getent` both use exit status as their answer, so only a failure to start the shell at all is an error
/// worth reporting.
async fn exists(runner: &impl ScriptRunner, script: &str) -> Result<bool, LeaseError> {
    Ok(runner.run(SHELL, script).await? == ScriptOutcome::Succeeded)
}

fn provisioning_failed(tool: &str, code: i32, stderr: &str) -> LeaseError {
    // Exit 1 from `useradd`/`groupadd` is what a non-root agent gets, and "permission denied" is far more actionable
    // than the raw code, so it is named when the message looks like it.
    if stderr.contains("Permission denied") || stderr.contains("must be run as root") {
        return LeaseError::Backend {
            message: format!("{tool} needs root inside the actor; the agent is not running as root ({stderr})"),
        };
    }
    LeaseError::Backend {
        message: format!("{tool} failed (exit {code}): {stderr}"),
    }
}

#[cfg(test)]
mod tests;
