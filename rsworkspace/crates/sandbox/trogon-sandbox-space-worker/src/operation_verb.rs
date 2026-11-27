/// The verb of a provider operation subject.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum OperationVerb {
    CheckpointRunning,
    Snapshot,
    GetDomain,
    Pause,
    Resume,
    Suspend,
    Wake,
    RunCommand,
    WriteFiles,
    ReadFile,
    FileOp,
    /// Announces a cancelled lease; the worker acts on it through its dispatch ledger.
    Cancel,
    /// Announces a purged lease; the worker acts on it through its dispatch ledger.
    Purge,
    /// Any other verb, kept as written.
    Other(String),
}

impl OperationVerb {
    /// The verb for one subject token.
    pub fn parse(token: &str) -> Self {
        match token {
            "checkpoint_running" => Self::CheckpointRunning,
            "snapshot" => Self::Snapshot,
            "get_domain" => Self::GetDomain,
            "pause" => Self::Pause,
            "resume" => Self::Resume,
            "suspend" => Self::Suspend,
            "wake" => Self::Wake,
            "run_command" => Self::RunCommand,
            "write_files" => Self::WriteFiles,
            "read_file" => Self::ReadFile,
            "file_op" => Self::FileOp,
            "cancel" => Self::Cancel,
            "purge" => Self::Purge,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The subject token of the verb.
    pub fn as_str(&self) -> &str {
        match self {
            Self::CheckpointRunning => "checkpoint_running",
            Self::Snapshot => "snapshot",
            Self::GetDomain => "get_domain",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Suspend => "suspend",
            Self::Wake => "wake",
            Self::RunCommand => "run_command",
            Self::WriteFiles => "write_files",
            Self::ReadFile => "read_file",
            Self::FileOp => "file_op",
            Self::Cancel => "cancel",
            Self::Purge => "purge",
            Self::Other(token) => token,
        }
    }
}
