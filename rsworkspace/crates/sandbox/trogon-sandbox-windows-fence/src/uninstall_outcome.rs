/// What an uninstall did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallOutcome {
    Removed,
    /// The user dismissed the UAC prompt.
    Cancelled,
}
