/// Which subscription a message arrived on, and so which handler answers it.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum Verb {
    Lease,
    Cancel,
    RunCommand,
    PollCommand,
    KillCommand,
    WriteStdin,
    WriteFiles,
    ReadFile,
    FileOp,
    GetDomain,
    CreateUser,
    CreateGroup,
    ModifyGroupMembership,
    CheckpointRunning,
    NetworkPolicyUpdate,
    SnapshotExport,
    PurgeNamed,
}
