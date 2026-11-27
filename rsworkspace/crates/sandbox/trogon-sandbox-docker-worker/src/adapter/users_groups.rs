use super::*;

impl DockerWorkerProvider {
    /// Creates a per-sandbox Linux user in `lease_id`'s running container,
    /// mirroring Vercel Sandbox's multi-agent per-user isolation model. Probes
    /// via `id -u` first so a repeated call for the same lease/username is
    /// idempotent rather than failing on `useradd`'s "already exists" error.
    ///
    /// Before running `useradd`, also probes that `useradd` and `/bin/bash`
    /// are actually present in the image, returning
    /// `CreateSandboxUserResponse::MissingDependency` instead of attempting
    /// (and failing) the script when either is absent.
    pub async fn create_user_for_lease(
        &self,
        lease_id: &LeaseId,
        username: &SandboxUserName,
    ) -> anyhow::Result<CreateSandboxUserResponse> {
        let container_id = self.active_container_id(lease_id).await?;
        let home_dir = home_dir_for_user(username);

        let probe = Self::exec_collect(
            &self.docker,
            &container_id,
            vec!["id".to_string(), "-u".to_string(), username.as_str().to_string()],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if probe.exit_code == 0 {
            return Ok(CreateSandboxUserResponse::AlreadyExists { home_dir });
        }

        if let Some(missing) = Self::missing_dependency(&self.docker, &container_id, "useradd").await? {
            return Ok(CreateSandboxUserResponse::MissingDependency { tool: missing });
        }

        let script = format!(
            "useradd -m -s /bin/bash '{name}' && chmod 770 '{home}'",
            name = username.as_str(),
            home = home_dir
        );
        let out = Self::exec_collect(
            &self.docker,
            &container_id,
            vec!["sh".to_string(), "-c".to_string(), script],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if out.exit_code != 0 {
            return Ok(CreateSandboxUserResponse::Denied(
                trogon_sandbox_contract::LeaseError::Backend {
                    message: format!(
                        "useradd for {} failed (exit {}): {}",
                        username.as_str(),
                        out.exit_code,
                        String::from_utf8_lossy(&out.stderr)
                    ),
                },
            ));
        }

        Ok(CreateSandboxUserResponse::Created { home_dir })
    }

    /// Creates a per-sandbox Linux group in `lease_id`'s running container.
    /// Probes via `getent group` first for the same idempotency reason as
    /// [`Self::create_user_for_lease`]. The shared directory is created
    /// setgid (`2770`) so files written into it by any group member inherit
    /// the group rather than the writer's primary group.
    ///
    /// Before running `groupadd`, also probes that `groupadd` and `/bin/bash`
    /// are present, returning
    /// `CreateSandboxGroupResponse::MissingDependency` instead of attempting
    /// the script when either is absent.
    pub async fn create_group_for_lease(
        &self,
        lease_id: &LeaseId,
        groupname: &SandboxGroupName,
    ) -> anyhow::Result<CreateSandboxGroupResponse> {
        let container_id = self.active_container_id(lease_id).await?;
        let shared_dir = shared_dir_for_group(groupname);

        let probe = Self::exec_collect(
            &self.docker,
            &container_id,
            vec![
                "getent".to_string(),
                "group".to_string(),
                groupname.as_str().to_string(),
            ],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if probe.exit_code == 0 {
            return Ok(CreateSandboxGroupResponse::AlreadyExists { shared_dir });
        }

        if let Some(missing) = Self::missing_dependency(&self.docker, &container_id, "groupadd").await? {
            return Ok(CreateSandboxGroupResponse::MissingDependency { tool: missing });
        }

        let script = format!(
            "groupadd '{name}' && mkdir -p '{dir}' && chgrp '{name}' '{dir}' && chmod 2770 '{dir}'",
            name = groupname.as_str(),
            dir = shared_dir
        );
        let out = Self::exec_collect(
            &self.docker,
            &container_id,
            vec!["sh".to_string(), "-c".to_string(), script],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if out.exit_code != 0 {
            return Ok(CreateSandboxGroupResponse::Denied(
                trogon_sandbox_contract::LeaseError::Backend {
                    message: format!(
                        "groupadd for {} failed (exit {}): {}",
                        groupname.as_str(),
                        out.exit_code,
                        String::from_utf8_lossy(&out.stderr)
                    ),
                },
            ));
        }

        Ok(CreateSandboxGroupResponse::Created { shared_dir })
    }

    /// Adds or removes `username`'s membership in `groupname` inside
    /// `lease_id`'s running container.
    ///
    /// Before applying the change, probes that the tool the requested
    /// `action` needs (`usermod` for `Add`, `gpasswd` for `Remove`) and
    /// `/bin/bash` are present, returning
    /// `ModifyGroupMembershipResponse::MissingDependency` instead of
    /// attempting the change when either is absent.
    pub async fn modify_group_membership_for_lease(
        &self,
        lease_id: &LeaseId,
        username: &SandboxUserName,
        groupname: &SandboxGroupName,
        action: GroupMembershipAction,
    ) -> anyhow::Result<ModifyGroupMembershipResponse> {
        let container_id = self.active_container_id(lease_id).await?;
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
        let required_tool = match action {
            GroupMembershipAction::Add => "usermod",
            GroupMembershipAction::Remove => "gpasswd",
        };
        if let Some(missing) = Self::missing_dependency(&self.docker, &container_id, required_tool).await? {
            return Ok(ModifyGroupMembershipResponse::MissingDependency { tool: missing });
        }

        let out = Self::exec_collect(
            &self.docker,
            &container_id,
            command,
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if out.exit_code != 0 {
            return Ok(ModifyGroupMembershipResponse::Denied(
                trogon_sandbox_contract::LeaseError::Backend {
                    message: format!(
                        "group membership update failed (exit {}): {}",
                        out.exit_code,
                        String::from_utf8_lossy(&out.stderr)
                    ),
                },
            ));
        }

        Ok(ModifyGroupMembershipResponse::Applied)
    }

    /// Probes whether `tool` (via `command -v`) and `/bin/bash` (via
    /// `test -x`) are both present in `container_id`, returning the name of
    /// whichever is missing first (`tool` is checked before `/bin/bash`), or
    /// `None` if both are present.
    pub(super) async fn missing_dependency(
        docker: &Docker,
        container_id: &str,
        tool: &str,
    ) -> anyhow::Result<Option<String>> {
        let probe_script = format!("command -v {tool} >/dev/null 2>&1");
        let tool_probe = Self::exec_collect(
            docker,
            container_id,
            vec!["sh".to_string(), "-c".to_string(), probe_script],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if tool_probe.exit_code != 0 {
            return Ok(Some(tool.to_string()));
        }

        let bash_probe = Self::exec_collect(
            docker,
            container_id,
            vec!["sh".to_string(), "-c".to_string(), "test -x /bin/bash".to_string()],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if bash_probe.exit_code != 0 {
            return Ok(Some("bash".to_string()));
        }

        Ok(None)
    }
}
