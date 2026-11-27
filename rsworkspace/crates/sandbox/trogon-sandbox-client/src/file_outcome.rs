/// What moving one file did.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FileOutcome {
    pub(crate) bytes_transferred: u64,
    pub(crate) resumed: bool,
    pub(crate) skipped: bool,
}
