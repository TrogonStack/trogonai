use clap::ValueEnum;

/// Which worker serves leases.
#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum WorkerKind {
    /// Docker when the daemon answers, otherwise subprocess when an agent
    /// binary is given on Linux or macOS, otherwise the in-memory worker.
    Auto,
    Docker,
    Subprocess,
    /// Map-backed filesystem and scripted commands, no containers.
    Memory,
}

impl WorkerKind {
    /// Picks the concrete worker. The caller probes `docker_available` so this
    /// stays a pure function.
    pub fn resolve(self, docker_available: bool, has_agent_binary: bool, subprocess_supported: bool) -> Self {
        match self {
            Self::Auto if docker_available => Self::Docker,
            Self::Auto if subprocess_supported && has_agent_binary => Self::Subprocess,
            Self::Auto => Self::Memory,
            other => other,
        }
    }

    pub(crate) fn binary_name(self) -> &'static str {
        match self {
            Self::Docker => "trogon-sandbox-docker-worker",
            Self::Subprocess => "trogon-sandbox-subprocess-worker",
            Self::Memory | Self::Auto => "trogon-sandbox-memory-worker",
        }
    }
}
