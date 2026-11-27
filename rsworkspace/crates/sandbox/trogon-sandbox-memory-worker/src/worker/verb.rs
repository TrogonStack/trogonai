use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, GetSandboxDomainResponse, LeaseError,
    ModifyGroupMembershipResponse, NetworkPolicyUpdateResponse, SnapshotExportError, SnapshotExportOutcome,
    WriteStdinResponse,
};

/// A verb on a held lease's subject, by its wire token.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Verb {
    /// Served by the terminal hub.
    Terminal,
    /// Modelled by the worker.
    Served(ServedVerb),
    /// Answered with a typed refusal.
    Unsupported(UnsupportedVerb),
}

/// The verbs the memory worker models.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ServedVerb {
    RunCommand,
    PollCommand,
    KillCommand,
    StopCommand,
    RestartCommand,
    ListProcesses,
    WaitReady,
    WriteFiles,
    ReadFile,
    FileOp,
    Pause,
    Resume,
    Suspend,
    Wake,
    NativeHandle,
}

/// The verbs the memory worker refuses.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum UnsupportedVerb {
    CheckpointRunning,
    NetworkPolicyUpdate,
    WriteStdin,
    CreateUser,
    CreateGroup,
    ModifyGroupMembership,
    GetDomain,
}

impl Verb {
    /// The verb named by `token`, if the worker knows it.
    pub(crate) fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "terminal" => Self::Terminal,
            "run_command" => Self::Served(ServedVerb::RunCommand),
            "poll_command" => Self::Served(ServedVerb::PollCommand),
            "kill_command" => Self::Served(ServedVerb::KillCommand),
            "stop_command" => Self::Served(ServedVerb::StopCommand),
            "restart_command" => Self::Served(ServedVerb::RestartCommand),
            "list_processes" => Self::Served(ServedVerb::ListProcesses),
            "wait_ready" => Self::Served(ServedVerb::WaitReady),
            "write_files" => Self::Served(ServedVerb::WriteFiles),
            "read_file" => Self::Served(ServedVerb::ReadFile),
            "file_op" => Self::Served(ServedVerb::FileOp),
            "pause" => Self::Served(ServedVerb::Pause),
            "resume" => Self::Served(ServedVerb::Resume),
            "suspend" => Self::Served(ServedVerb::Suspend),
            "wake" => Self::Served(ServedVerb::Wake),
            "native_handle" => Self::Served(ServedVerb::NativeHandle),
            "checkpoint_running" => Self::Unsupported(UnsupportedVerb::CheckpointRunning),
            "network_policy_update" => Self::Unsupported(UnsupportedVerb::NetworkPolicyUpdate),
            "write_stdin" => Self::Unsupported(UnsupportedVerb::WriteStdin),
            "create_user" => Self::Unsupported(UnsupportedVerb::CreateUser),
            "create_group" => Self::Unsupported(UnsupportedVerb::CreateGroup),
            "modify_group_membership" => Self::Unsupported(UnsupportedVerb::ModifyGroupMembership),
            "get_domain" => Self::Unsupported(UnsupportedVerb::GetDomain),
            _ => return None,
        })
    }
}

impl UnsupportedVerb {
    /// The typed reply for a verb this worker does not model.
    pub(crate) fn reply(self) -> serde_json::Result<Vec<u8>> {
        let denied = LeaseError::Backend {
            message: "not supported by the memory worker".to_string(),
        };
        match self {
            Self::CheckpointRunning => {
                serde_json::to_vec(&SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
                    message: denied.to_string(),
                }))
            }
            Self::NetworkPolicyUpdate => serde_json::to_vec(&NetworkPolicyUpdateResponse::Denied(denied)),
            Self::WriteStdin => serde_json::to_vec(&WriteStdinResponse::Unsupported),
            Self::CreateUser => serde_json::to_vec(&CreateSandboxUserResponse::Denied(denied)),
            Self::CreateGroup => serde_json::to_vec(&CreateSandboxGroupResponse::Denied(denied)),
            Self::ModifyGroupMembership => serde_json::to_vec(&ModifyGroupMembershipResponse::Denied(denied)),
            Self::GetDomain => serde_json::to_vec(&GetSandboxDomainResponse::Unsupported),
        }
    }
}
