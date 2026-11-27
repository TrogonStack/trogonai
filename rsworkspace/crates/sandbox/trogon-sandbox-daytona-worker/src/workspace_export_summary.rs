/// What an export pushed to the volume.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct WorkspaceExportSummary {
    pub objects: u64,
    pub bytes: u64,
}
