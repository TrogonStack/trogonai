#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum LspState {
    Idle,
    Starting,
    Running,
    Stopping,
    Stopped,
}

impl LspState {
    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}
