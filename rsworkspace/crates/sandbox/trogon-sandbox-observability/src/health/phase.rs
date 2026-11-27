/// Where the process is in its life. Stored as one atomic byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Starting,
    Running,
    ShuttingDown,
}

impl From<Phase> for u8 {
    fn from(phase: Phase) -> Self {
        match phase {
            Phase::Starting => 0,
            Phase::Running => 1,
            Phase::ShuttingDown => 2,
        }
    }
}

impl From<u8> for Phase {
    fn from(raw: u8) -> Self {
        match raw {
            0 => Self::Starting,
            1 => Self::Running,
            _ => Self::ShuttingDown,
        }
    }
}
