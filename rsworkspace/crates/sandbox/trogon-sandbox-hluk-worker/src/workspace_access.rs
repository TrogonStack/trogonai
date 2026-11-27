/// Whether the guest may write to its workspace.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WorkspaceAccess {
    ReadWrite,
    ReadOnly,
}
