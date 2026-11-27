/// How a detached command's watch ended.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DetachedExit {
    /// The command exited with this code.
    Exited(i32),
    /// The agent no longer knows the execution, refused the poll, or stopped
    /// answering, so the exit code cannot be learned.
    Unknown,
}

impl DetachedExit {
    /// The exit code when the command was seen exiting.
    pub fn exit_code(self) -> Option<i32> {
        match self {
            Self::Exited(code) => Some(code),
            Self::Unknown => None,
        }
    }
}
