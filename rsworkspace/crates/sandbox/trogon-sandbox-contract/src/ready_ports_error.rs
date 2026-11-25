/// A port list does not satisfy the rules [`ReadyPorts`](crate::ReadyPorts) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ReadyPortsError {
    #[error("at most {max} ports may be checked")]
    TooMany { max: usize },
    #[error("port 0 cannot be checked")]
    ZeroPort,
    #[error("duplicate ports")]
    Duplicate,
}
