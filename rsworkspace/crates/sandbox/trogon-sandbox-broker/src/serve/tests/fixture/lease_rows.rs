use std::error::Error;

use serde_json::json;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CreateSandboxGroupRequest, CreateSandboxUserRequest, GetSandboxDomainRequest,
    GroupMembershipAction, KillCommandRequest, LaunchKey, ListProcessesRequest, ModifyGroupMembershipRequest,
    NativeHandleRequest, NetworkPolicy, NetworkPolicyUpdateRequest, PollCommandRequest, ReadLogsRequest,
    RestartCommandRequest, SandboxTags, SshAccessRequest, StdinChunk, StdinEof, StdinSeq, StopCommandRequest,
    TerminalId, TerminalOp, TerminalRequest, UpdateLeaseTagsRequest, WaitReadyRequest, WaitSandboxReadyRequest,
    WriteStdinRequest,
};
use trogon_sandbox_protocol::subjects;

use super::{Fixture, Row, body};

impl Fixture {
    pub(super) fn command_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let (lease_id, execution_id) = (self.lease.clone(), self.execution.clone());
        let lease = &self.lease;
        let tags = UpdateLeaseTagsRequest {
            lease_id: lease_id.clone(),
            tags: SandboxTags::default(),
        };
        let network = NetworkPolicyUpdateRequest {
            lease_id: lease_id.clone(),
            network_policy: NetworkPolicy::AllowAll,
        };
        let run = json!({"lease_id": lease.as_str(), "command": ["true"]});
        let kill = KillCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            signal: Default::default(),
        };
        let cancel_launch = CancelCommandLaunchRequest {
            lease_id: lease_id.clone(),
            launch_key: LaunchKey::new("launch-1")?,
        };
        let stop = StopCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            grace_seconds: 1,
        };
        let restart = RestartCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            grace_seconds: 1,
        };
        let logs = ReadLogsRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            after_seq: 0,
            max_bytes: None,
            wait_ms: None,
        };
        let stdin = WriteStdinRequest::new(
            lease_id.clone(),
            execution_id.clone(),
            StdinSeq::new(0),
            StdinChunk::new(b"x".to_vec())?,
            StdinEof::Keep,
        )?;
        let poll = PollCommandRequest {
            lease_id: lease_id.clone(),
            execution_id,
        };
        Ok(vec![
            (subjects::lease_tags_update(lease), body(&tags)?),
            (subjects::lease_network_policy_update(lease), body(&network)?),
            (subjects::lease_extend_timeout(lease), self.extend_timeout()?),
            (subjects::lease_run_command(lease), body(&run)?),
            (subjects::lease_poll_command(lease), body(&poll)?),
            (subjects::lease_kill_command(lease), body(&kill)?),
            (subjects::lease_cancel_command_launch(lease), body(&cancel_launch)?),
            (subjects::lease_stop_command(lease), body(&stop)?),
            (subjects::lease_restart_command(lease), body(&restart)?),
            (subjects::lease_read_logs(lease), body(&logs)?),
            (subjects::lease_write_stdin(lease), body(&stdin)?),
            (
                subjects::lease_list_processes(lease),
                body(&ListProcessesRequest { lease_id })?,
            ),
        ])
    }

    pub(super) fn sandbox_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let lease_id = self.lease.clone();
        let lease = &self.lease;
        let wait_ready = WaitReadyRequest {
            lease_id: lease_id.clone(),
            execution_id: self.execution.clone(),
            timeout_seconds: 1,
        };
        let wait_sandbox = WaitSandboxReadyRequest {
            lease_id: lease_id.clone(),
            ports: Default::default(),
            timeout_seconds: 1,
        };
        let terminal = TerminalRequest {
            lease_id: lease_id.clone(),
            terminal_id: TerminalId::new("term-1")?,
            op: TerminalOp::Close,
        };
        let domain = GetSandboxDomainRequest {
            lease_id: lease_id.clone(),
            port: 8080,
            wait_ms: None,
        };
        let ssh = SshAccessRequest {
            lease_id: lease_id.clone(),
            ttl_seconds: None,
        };
        let user = CreateSandboxUserRequest {
            lease_id: lease_id.clone(),
            username: self.user.clone(),
        };
        let group = CreateSandboxGroupRequest {
            lease_id: lease_id.clone(),
            groupname: self.group.clone(),
        };
        let membership = ModifyGroupMembershipRequest {
            lease_id: lease_id.clone(),
            username: self.user.clone(),
            groupname: self.group.clone(),
            action: GroupMembershipAction::Add,
        };
        Ok(vec![
            (subjects::lease_wait_ready(lease), body(&wait_ready)?),
            (subjects::lease_wait_sandbox_ready(lease), body(&wait_sandbox)?),
            (subjects::lease_terminal(lease), body(&terminal)?),
            (subjects::lease_get_domain(lease), body(&domain)?),
            (
                subjects::lease_native_handle(lease),
                body(&NativeHandleRequest { lease_id })?,
            ),
            (subjects::lease_ssh_access(lease), body(&ssh)?),
            (subjects::lease_create_user(lease), body(&user)?),
            (subjects::lease_create_group(lease), body(&group)?),
            (subjects::lease_modify_group_membership(lease), body(&membership)?),
        ])
    }
}
