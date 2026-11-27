use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, GroupMembershipAction, LeaseError, LeaseId,
    ModifyGroupMembershipResponse, SandboxGroupName, SandboxUserName,
};
use trogon_sandbox_protocol::{home_dir_for_user, shared_dir_for_group};

use crate::{K8sWorkerError, K8sWorkerProvider};

impl K8sWorkerProvider {
    /// Creates a per-sandbox Linux user in `lease_id`'s running pod.
    /// Probes `useradd` and `/bin/bash` availability first, then `id -u` for
    /// idempotency, before running `useradd` itself. Returns `None` if
    /// `lease_id` has no active lease on this worker, matching
    /// [`Self::exec_in_pod_for_lease`].
    pub async fn create_user_for_lease(
        &self,
        lease_id: &LeaseId,
        username: &SandboxUserName,
    ) -> Option<Result<CreateSandboxUserResponse, K8sWorkerError>> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        Some(self.create_user_for_sandbox(&sandbox_id, username).await)
    }

    async fn create_user_for_sandbox(
        &self,
        sandbox_id: &str,
        username: &SandboxUserName,
    ) -> Result<CreateSandboxUserResponse, K8sWorkerError> {
        if !self.tool_present(sandbox_id, "useradd").await? {
            return Ok(CreateSandboxUserResponse::MissingDependency {
                tool: "useradd".to_string(),
            });
        }
        if !self.tool_present(sandbox_id, "/bin/bash").await? {
            return Ok(CreateSandboxUserResponse::MissingDependency {
                tool: "/bin/bash".to_string(),
            });
        }

        let home_dir = home_dir_for_user(username);

        let probe = vec!["id".to_string(), "-u".to_string(), username.as_str().to_string()];
        let probe_out = self.inner.exec_in_pod(sandbox_id, probe).await?;
        if probe_out.exit_code == 0 {
            return Ok(CreateSandboxUserResponse::AlreadyExists { home_dir });
        }

        let script = format!(
            "useradd -m -s /bin/bash '{name}' && chmod 770 '{home}'",
            name = username.as_str(),
            home = home_dir
        );
        let out = self
            .inner
            .exec_in_pod(sandbox_id, vec!["sh".to_string(), "-c".to_string(), script])
            .await?;
        if out.exit_code != 0 {
            return Ok(CreateSandboxUserResponse::Denied(LeaseError::Backend {
                message: format!(
                    "useradd for {} failed (exit {}): {}",
                    username.as_str(),
                    out.exit_code,
                    String::from_utf8_lossy(&out.stderr)
                ),
            }));
        }

        Ok(CreateSandboxUserResponse::Created { home_dir })
    }

    /// Creates a per-sandbox Linux group in `lease_id`'s running pod. Probes `groupadd`
    /// availability first, then `getent group` for idempotency, before running
    /// `groupadd` itself. Returns `None` if `lease_id` has no active lease on
    /// this worker, matching [`Self::exec_in_pod_for_lease`].
    pub async fn create_group_for_lease(
        &self,
        lease_id: &LeaseId,
        groupname: &SandboxGroupName,
    ) -> Option<Result<CreateSandboxGroupResponse, K8sWorkerError>> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        Some(self.create_group_for_sandbox(&sandbox_id, groupname).await)
    }

    async fn create_group_for_sandbox(
        &self,
        sandbox_id: &str,
        groupname: &SandboxGroupName,
    ) -> Result<CreateSandboxGroupResponse, K8sWorkerError> {
        if !self.tool_present(sandbox_id, "groupadd").await? {
            return Ok(CreateSandboxGroupResponse::MissingDependency {
                tool: "groupadd".to_string(),
            });
        }
        if !self.tool_present(sandbox_id, "/bin/bash").await? {
            return Ok(CreateSandboxGroupResponse::MissingDependency {
                tool: "/bin/bash".to_string(),
            });
        }

        let shared_dir = shared_dir_for_group(groupname);

        let probe = vec![
            "getent".to_string(),
            "group".to_string(),
            groupname.as_str().to_string(),
        ];
        let probe_out = self.inner.exec_in_pod(sandbox_id, probe).await?;
        if probe_out.exit_code == 0 {
            return Ok(CreateSandboxGroupResponse::AlreadyExists { shared_dir });
        }

        let script = format!(
            "groupadd '{name}' && mkdir -p '{dir}' && chgrp '{name}' '{dir}' && chmod 2770 '{dir}'",
            name = groupname.as_str(),
            dir = shared_dir
        );
        let out = self
            .inner
            .exec_in_pod(sandbox_id, vec!["sh".to_string(), "-c".to_string(), script])
            .await?;
        if out.exit_code != 0 {
            return Ok(CreateSandboxGroupResponse::Denied(LeaseError::Backend {
                message: format!(
                    "groupadd for {} failed (exit {}): {}",
                    groupname.as_str(),
                    out.exit_code,
                    String::from_utf8_lossy(&out.stderr)
                ),
            }));
        }

        Ok(CreateSandboxGroupResponse::Created { shared_dir })
    }

    /// Adds or removes `username`'s membership in `groupname` inside
    /// `lease_id`'s running pod. Probes for `usermod` (Add) or
    /// `gpasswd` (Remove) availability first. Returns `None` if `lease_id`
    /// has no active lease on this worker, matching
    /// [`Self::exec_in_pod_for_lease`].
    pub async fn modify_group_membership_for_lease(
        &self,
        lease_id: &LeaseId,
        username: &SandboxUserName,
        groupname: &SandboxGroupName,
        action: GroupMembershipAction,
    ) -> Option<Result<ModifyGroupMembershipResponse, K8sWorkerError>> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        Some(
            self.modify_group_membership_for_sandbox(&sandbox_id, username, groupname, action)
                .await,
        )
    }

    async fn modify_group_membership_for_sandbox(
        &self,
        sandbox_id: &str,
        username: &SandboxUserName,
        groupname: &SandboxGroupName,
        action: GroupMembershipAction,
    ) -> Result<ModifyGroupMembershipResponse, K8sWorkerError> {
        let tool = match action {
            GroupMembershipAction::Add => "usermod",
            GroupMembershipAction::Remove => "gpasswd",
        };
        if !self.tool_present(sandbox_id, tool).await? {
            return Ok(ModifyGroupMembershipResponse::MissingDependency { tool: tool.to_string() });
        }

        let command = match action {
            GroupMembershipAction::Add => vec![
                "usermod".to_string(),
                "-aG".to_string(),
                groupname.as_str().to_string(),
                username.as_str().to_string(),
            ],
            GroupMembershipAction::Remove => vec![
                "gpasswd".to_string(),
                "-d".to_string(),
                username.as_str().to_string(),
                groupname.as_str().to_string(),
            ],
        };
        let out = self.inner.exec_in_pod(sandbox_id, command).await?;
        if out.exit_code != 0 {
            return Ok(ModifyGroupMembershipResponse::Denied(LeaseError::Backend {
                message: format!(
                    "group membership update failed (exit {}): {}",
                    out.exit_code,
                    String::from_utf8_lossy(&out.stderr)
                ),
            }));
        }

        Ok(ModifyGroupMembershipResponse::Applied)
    }
}
