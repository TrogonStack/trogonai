use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::subjects::provider_token;
use trogon_sandbox_ssh_gateway::{SSH_ATTACH_FILTER, SSH_DETACH_FILTER};

/// A per-lease verb the broker sends to every worker, of which only the one holding the lease answers.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum K8sVerb {
    NetworkPolicyUpdate,
    CheckpointRunning,
    RunCommand,
    PollCommand,
    KillCommand,
    StopCommand,
    RestartCommand,
    WriteStdin,
    WriteFiles,
    ReadFile,
    FileOp,
    CreateUser,
    CreateGroup,
    ModifyGroupMembership,
    GetDomain,
    SshAttach,
    SshDetach,
    Terminal,
    Rollback,
}

impl K8sVerb {
    /// Every verb the worker answers.
    pub const ALL: [K8sVerb; 19] = [
        Self::NetworkPolicyUpdate,
        Self::CheckpointRunning,
        Self::RunCommand,
        Self::PollCommand,
        Self::KillCommand,
        Self::StopCommand,
        Self::RestartCommand,
        Self::WriteStdin,
        Self::WriteFiles,
        Self::ReadFile,
        Self::FileOp,
        Self::CreateUser,
        Self::CreateGroup,
        Self::ModifyGroupMembership,
        Self::GetDomain,
        Self::SshAttach,
        Self::SshDetach,
        Self::Terminal,
        Self::Rollback,
    ];

    /// The name of the verb in logs and in its subject.
    pub fn token(self) -> &'static str {
        match self {
            Self::NetworkPolicyUpdate => "network_policy_update",
            Self::CheckpointRunning => "checkpoint_running",
            Self::RunCommand => "run_command",
            Self::PollCommand => "poll_command",
            Self::KillCommand => "kill_command",
            Self::StopCommand => "stop_command",
            Self::RestartCommand => "restart_command",
            Self::WriteStdin => "write_stdin",
            Self::WriteFiles => "write_files",
            Self::ReadFile => "read_file",
            Self::FileOp => "file_op",
            Self::CreateUser => "create_user",
            Self::CreateGroup => "create_group",
            Self::ModifyGroupMembership => "modify_group_membership",
            Self::GetDomain => "get_domain",
            Self::SshAttach => "ssh_attach",
            Self::SshDetach => "ssh_detach",
            Self::Terminal => "terminal",
            Self::Rollback => "rollback",
        }
    }

    /// The wildcard subject that carries this verb for every lease.
    pub fn subject(self) -> String {
        match self {
            Self::SshAttach => SSH_ATTACH_FILTER.to_owned(),
            Self::SshDetach => SSH_DETACH_FILTER.to_owned(),
            verb => format!(
                "sandbox.provider.{}.{}.>",
                provider_token(ProviderKind::K8sMicroVm),
                verb.token()
            ),
        }
    }
}

#[cfg(test)]
mod tests;
