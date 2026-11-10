pub(crate) enum FaultOrigin {
    HistoryRead,
    HistoryIntegrity,
    ExecutionReadStream,
    ExecutionAppend,
    ExecutionIntegrity,
    PortBackend,
    Context,
}

impl FaultOrigin {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::HistoryRead => "history.read",
            Self::HistoryIntegrity => "history.integrity",
            Self::ExecutionReadStream => "execution.read_stream",
            Self::ExecutionAppend => "execution.append",
            Self::ExecutionIntegrity => "execution.integrity",
            Self::PortBackend => "port.backend",
            Self::Context => "context",
        }
    }
}
