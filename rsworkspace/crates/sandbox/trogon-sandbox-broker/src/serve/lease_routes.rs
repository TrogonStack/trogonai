//! Routes whose request names the lease it acts on, authorized against the lease owner.

use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CreateSandboxGroupRequest, CreateSandboxGroupResponse,
    CreateSandboxUserRequest, CreateSandboxUserResponse, ExtendLeaseTimeoutRequest, ExtendLeaseTimeoutResponse,
    GetSandboxDomainRequest, GetSandboxDomainResponse, KillCommandRequest, KillCommandResponse, ListProcessesRequest,
    ListProcessesResponse, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse, NativeHandleRequest,
    NativeHandleResponse, NetworkPolicyUpdateRequest, NetworkPolicyUpdateResponse, PollCommandRequest,
    PollCommandResponse, ReadLogsRequest, ReadLogsResponse, RestartCommandRequest, RestartCommandResponse,
    RunCommandRequest, RunCommandResponse, SshAccessRequest, SshAccessResponse, StopCommandRequest,
    StopCommandResponse, TerminalRequest, TerminalResponse, UpdateLeaseTagsRequest, UpdateLeaseTagsResponse,
    WaitReadyRequest, WaitReadyResponse, WaitSandboxReadyRequest, WaitSandboxReadyResponse, WriteStdinRequest,
    WriteStdinResponse,
};
use trogon_sandbox_protocol::subjects::{
    LEASE_CANCEL_COMMAND_LAUNCH_FILTER, LEASE_CREATE_GROUP_FILTER, LEASE_CREATE_USER_FILTER,
    LEASE_EXTEND_TIMEOUT_FILTER, LEASE_GET_DOMAIN_FILTER, LEASE_KILL_COMMAND_FILTER, LEASE_LIST_PROCESSES_FILTER,
    LEASE_MODIFY_GROUP_MEMBERSHIP_FILTER, LEASE_NATIVE_HANDLE_FILTER, LEASE_NETWORK_POLICY_UPDATE_FILTER,
    LEASE_POLL_COMMAND_FILTER, LEASE_READ_LOGS_FILTER, LEASE_RESTART_COMMAND_FILTER, LEASE_RUN_COMMAND_FILTER,
    LEASE_SSH_ACCESS_FILTER, LEASE_STOP_COMMAND_FILTER, LEASE_TAGS_UPDATE_FILTER, LEASE_TERMINAL_FILTER,
    LEASE_WAIT_READY_FILTER, LEASE_WAIT_SANDBOX_READY_FILTER, LEASE_WRITE_STDIN_FILTER,
};

use super::handlers::command_reply;
use super::lease_route::LeaseRoute;
use super::router::Router;
use super::serve_error::ServeError;
use crate::auth::ClientVerb;

/// Subscribes every lease scoped route.
pub(crate) async fn register(router: &Router) -> Result<(), ServeError> {
    register_lease_settings(router).await?;
    register_commands(router).await?;
    register_sandbox_access(router).await
}

async fn register_lease_settings(router: &Router) -> Result<(), ServeError> {
    router
        .lease(
            LEASE_TAGS_UPDATE_FILTER,
            LeaseRoute::new(
                ClientVerb::TagsUpdate,
                |r: &UpdateLeaseTagsRequest| &r.lease_id,
                UpdateLeaseTagsResponse::Denied,
            )
            .replying_to_decode_errors(),
            |c, r| async move { c.broker.handle_update_lease_tags(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_NETWORK_POLICY_UPDATE_FILTER,
            LeaseRoute::new(
                ClientVerb::NetworkPolicyUpdate,
                |r: &NetworkPolicyUpdateRequest| &r.lease_id,
                NetworkPolicyUpdateResponse::Denied,
            )
            .replying_to_decode_errors(),
            |c, r| async move { c.broker.handle_network_policy_update(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_EXTEND_TIMEOUT_FILTER,
            LeaseRoute::new(
                ClientVerb::ExtendTimeout,
                |r: &ExtendLeaseTimeoutRequest| &r.lease_id,
                ExtendLeaseTimeoutResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_extend_lease_timeout(r).await },
        )
        .await
}

async fn register_commands(router: &Router) -> Result<(), ServeError> {
    router
        .lease(
            LEASE_RUN_COMMAND_FILTER,
            LeaseRoute::new(
                ClientVerb::RunCommand,
                |r: &RunCommandRequest| &r.lease_id,
                RunCommandResponse::Denied,
            ),
            command_reply::run,
        )
        .await?;
    router
        .lease(
            LEASE_POLL_COMMAND_FILTER,
            LeaseRoute::new(
                ClientVerb::PollCommand,
                |r: &PollCommandRequest| &r.lease_id,
                PollCommandResponse::Denied,
            ),
            command_reply::poll,
        )
        .await?;
    router
        .lease(
            LEASE_KILL_COMMAND_FILTER,
            LeaseRoute::new(
                ClientVerb::KillCommand,
                |r: &KillCommandRequest| &r.lease_id,
                KillCommandResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_kill_command(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_CANCEL_COMMAND_LAUNCH_FILTER,
            LeaseRoute::new(
                ClientVerb::CancelCommandLaunch,
                |r: &CancelCommandLaunchRequest| &r.lease_id,
                CancelCommandLaunchResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_cancel_command_launch(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_STOP_COMMAND_FILTER,
            LeaseRoute::new(
                ClientVerb::StopCommand,
                |r: &StopCommandRequest| &r.lease_id,
                StopCommandResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_stop_command(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_RESTART_COMMAND_FILTER,
            LeaseRoute::new(
                ClientVerb::RestartCommand,
                |r: &RestartCommandRequest| &r.lease_id,
                RestartCommandResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_restart_command(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_READ_LOGS_FILTER,
            LeaseRoute::new(
                ClientVerb::ReadLogs,
                |r: &ReadLogsRequest| &r.lease_id,
                ReadLogsResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_read_logs(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_WRITE_STDIN_FILTER,
            LeaseRoute::new(
                ClientVerb::WriteStdin,
                WriteStdinRequest::lease_id,
                WriteStdinResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_write_stdin(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_LIST_PROCESSES_FILTER,
            LeaseRoute::new(
                ClientVerb::ListProcesses,
                |r: &ListProcessesRequest| &r.lease_id,
                ListProcessesResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_list_processes(r).await },
        )
        .await
}

async fn register_sandbox_access(router: &Router) -> Result<(), ServeError> {
    router
        .lease(
            LEASE_WAIT_READY_FILTER,
            LeaseRoute::new(
                ClientVerb::WaitReady,
                |r: &WaitReadyRequest| &r.lease_id,
                WaitReadyResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_wait_ready(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_WAIT_SANDBOX_READY_FILTER,
            LeaseRoute::new(
                ClientVerb::WaitSandboxReady,
                |r: &WaitSandboxReadyRequest| &r.lease_id,
                WaitSandboxReadyResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_wait_sandbox_ready(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_TERMINAL_FILTER,
            LeaseRoute::new(
                ClientVerb::Terminal,
                |r: &TerminalRequest| &r.lease_id,
                TerminalResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_terminal(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_GET_DOMAIN_FILTER,
            LeaseRoute::new(
                ClientVerb::GetDomain,
                |r: &GetSandboxDomainRequest| &r.lease_id,
                GetSandboxDomainResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_get_sandbox_domain(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_NATIVE_HANDLE_FILTER,
            LeaseRoute::new(
                ClientVerb::NativeHandle,
                |r: &NativeHandleRequest| &r.lease_id,
                NativeHandleResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_native_handle(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_SSH_ACCESS_FILTER,
            LeaseRoute::new(
                ClientVerb::SshAccess,
                |r: &SshAccessRequest| &r.lease_id,
                SshAccessResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_ssh_access(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_CREATE_USER_FILTER,
            LeaseRoute::new(
                ClientVerb::CreateUser,
                |r: &CreateSandboxUserRequest| &r.lease_id,
                CreateSandboxUserResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_create_user(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_CREATE_GROUP_FILTER,
            LeaseRoute::new(
                ClientVerb::CreateGroup,
                |r: &CreateSandboxGroupRequest| &r.lease_id,
                CreateSandboxGroupResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_create_group(r).await },
        )
        .await?;
    router
        .lease(
            LEASE_MODIFY_GROUP_MEMBERSHIP_FILTER,
            LeaseRoute::new(
                ClientVerb::ModifyGroupMembership,
                |r: &ModifyGroupMembershipRequest| &r.lease_id,
                ModifyGroupMembershipResponse::Denied,
            ),
            |c, r| async move { c.broker.handle_modify_group_membership(r).await },
        )
        .await
}
