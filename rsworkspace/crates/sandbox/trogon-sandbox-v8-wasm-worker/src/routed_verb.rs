use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, FileOpResponse, GetSandboxDomainResponse,
    KillCommandResponse, LeaseError, ModifyGroupMembershipResponse, NetworkPolicyUpdateResponse, PollCommandResponse,
    ProviderKind, ReadFileResponse, RunCommandResponse, SnapshotExportError, SnapshotExportOutcome, WriteFilesResponse,
    WriteStdinResponse,
};
use trogon_sandbox_protocol::subjects::provider_token;

/// A per-lease verb the broker routes to a provider after the lease is granted.
///
/// A V8 or Wasm lease runs its guest to completion inside the lease call and then reclaims the
/// isolate, so no live sandbox is left to act on. Every verb is answered with its typed "not
/// supported" reply instead of being left to time out.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RoutedVerb {
    CheckpointRunning,
    NetworkPolicyUpdate,
    RunCommand,
    PollCommand,
    KillCommand,
    WriteStdin,
    WriteFiles,
    ReadFile,
    FileOp,
    CreateUser,
    CreateGroup,
    ModifyGroupMembership,
    GetDomain,
}

impl RoutedVerb {
    /// Every verb the worker answers.
    pub const ALL: [RoutedVerb; 13] = [
        Self::CheckpointRunning,
        Self::NetworkPolicyUpdate,
        Self::RunCommand,
        Self::PollCommand,
        Self::KillCommand,
        Self::WriteStdin,
        Self::WriteFiles,
        Self::ReadFile,
        Self::FileOp,
        Self::CreateUser,
        Self::CreateGroup,
        Self::ModifyGroupMembership,
        Self::GetDomain,
    ];

    /// The subject segment that names the verb.
    pub fn token(self) -> &'static str {
        match self {
            Self::CheckpointRunning => "checkpoint_running",
            Self::NetworkPolicyUpdate => "network_policy_update",
            Self::RunCommand => "run_command",
            Self::PollCommand => "poll_command",
            Self::KillCommand => "kill_command",
            Self::WriteStdin => "write_stdin",
            Self::WriteFiles => "write_files",
            Self::ReadFile => "read_file",
            Self::FileOp => "file_op",
            Self::CreateUser => "create_user",
            Self::CreateGroup => "create_group",
            Self::ModifyGroupMembership => "modify_group_membership",
            Self::GetDomain => "get_domain",
        }
    }

    /// The wildcard subject that carries this verb for every lease.
    pub fn subject(self) -> String {
        format!(
            "sandbox.provider.{}.{}.>",
            provider_token(ProviderKind::V8Wasm),
            self.token()
        )
    }

    fn denial(self) -> LeaseError {
        LeaseError::Backend {
            message: format!(
                "{} is not supported by v8_wasm: a lease runs its guest to completion and keeps no live sandbox",
                self.token()
            ),
        }
    }

    /// The JSON reply body for this verb, independent of the request payload.
    pub fn unsupported_reply(self) -> serde_json::Result<Vec<u8>> {
        let denied = self.denial();
        let message = denied.to_string();
        match self {
            Self::CheckpointRunning => {
                serde_json::to_vec(&SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
                    message,
                }))
            }
            Self::NetworkPolicyUpdate => serde_json::to_vec(&NetworkPolicyUpdateResponse::Denied(denied)),
            Self::RunCommand => serde_json::to_vec(&RunCommandResponse::Denied(denied)),
            Self::PollCommand => serde_json::to_vec(&PollCommandResponse::NotFound),
            Self::KillCommand => serde_json::to_vec(&KillCommandResponse::NotFound),
            Self::WriteStdin => serde_json::to_vec(&WriteStdinResponse::Unsupported),
            Self::WriteFiles => serde_json::to_vec(&WriteFilesResponse::Denied(denied)),
            Self::ReadFile => serde_json::to_vec(&ReadFileResponse::Denied(denied)),
            Self::FileOp => serde_json::to_vec(&FileOpResponse::Unsupported { message }),
            Self::CreateUser => serde_json::to_vec(&CreateSandboxUserResponse::Denied(denied)),
            Self::CreateGroup => serde_json::to_vec(&CreateSandboxGroupResponse::Denied(denied)),
            Self::ModifyGroupMembership => serde_json::to_vec(&ModifyGroupMembershipResponse::Denied(denied)),
            Self::GetDomain => serde_json::to_vec(&GetSandboxDomainResponse::Unsupported),
        }
    }
}
