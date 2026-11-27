/// Whether a launched command may start at once or waits for custody to be durable first.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum LaunchRelease {
    Immediate,
    Deferred,
}

impl LaunchRelease {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Deferred => "deferred",
        }
    }
}
