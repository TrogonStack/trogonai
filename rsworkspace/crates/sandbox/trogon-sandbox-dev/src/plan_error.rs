/// A [`DevPlan`](crate::DevPlan) could not be built from the flags and the binaries found.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum PlanError {
    #[error(
        "`nats-server` not found in --bin-dir, next to trogon-sandbox-dev, or on PATH. Install it (for example `brew install nats-server`) and put it on PATH."
    )]
    NatsServerNotFound,
    #[error(
        "`{name}` not found in --bin-dir, next to trogon-sandbox-dev, or on PATH. Build it with `cargo build --bin {name}` from the workspace root."
    )]
    BinaryNotFound { name: &'static str },
    #[error("--agent-binary (SANDBOX_AGENT_BINARY) is required for the subprocess worker")]
    AgentBinaryRequired,
}
