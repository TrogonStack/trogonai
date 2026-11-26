use std::time::Duration;

use trogon_sandbox_contract::{
    CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse,
    GroupMembershipAction, LeaseId, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse, SandboxGroupName,
    SandboxUserName,
};
use trogon_sandbox_protocol::subjects;

use crate::{ClientError, SandboxClient, TransportError};

const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(30);

impl SandboxClient {
    /// Creates a per-sandbox Linux user inside `lease_id`'s already-running sandbox.
    pub async fn create_user(
        &self,
        lease_id: &LeaseId,
        username: SandboxUserName,
    ) -> Result<CreateSandboxUserResponse, ClientError> {
        let request = CreateSandboxUserRequest {
            lease_id: lease_id.clone(),
            username,
        };
        let response: CreateSandboxUserResponse = self
            .send(&subjects::lease_create_user(lease_id), &request, ACCOUNT_TIMEOUT)
            .await?;
        match response {
            CreateSandboxUserResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Creates a per-sandbox Linux group inside `lease_id`'s already-running sandbox.
    pub async fn create_group(
        &self,
        lease_id: &LeaseId,
        groupname: SandboxGroupName,
    ) -> Result<CreateSandboxGroupResponse, ClientError> {
        let request = CreateSandboxGroupRequest {
            lease_id: lease_id.clone(),
            groupname,
        };
        let response: CreateSandboxGroupResponse = self
            .send(&subjects::lease_create_group(lease_id), &request, ACCOUNT_TIMEOUT)
            .await?;
        match response {
            CreateSandboxGroupResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Adds or removes `username`'s membership in `groupname` inside `lease_id`'s already-running sandbox.
    pub async fn modify_group_membership(
        &self,
        lease_id: &LeaseId,
        username: SandboxUserName,
        groupname: SandboxGroupName,
        action: GroupMembershipAction,
    ) -> Result<(), ClientError> {
        let request = ModifyGroupMembershipRequest {
            lease_id: lease_id.clone(),
            username,
            groupname,
            action,
        };
        let response: ModifyGroupMembershipResponse = self
            .send(
                &subjects::lease_modify_group_membership(lease_id),
                &request,
                ACCOUNT_TIMEOUT,
            )
            .await?;
        match response {
            ModifyGroupMembershipResponse::Applied => Ok(()),
            ModifyGroupMembershipResponse::MissingDependency { tool } => {
                Err(ClientError::Transport(TransportError::MissingDependency { tool }))
            }
            ModifyGroupMembershipResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }
}
