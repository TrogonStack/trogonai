use trogon_sandbox_actor_agent::GuestProtocolError;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// The router or the agent never answered. Usually means the actor is not up yet.
    #[error("agent at {target} is unreachable: {message}")]
    Unreachable { target: String, message: String },
    /// The agent answered with a status its contract does not define.
    #[error("agent at {target} returned HTTP {status}: {body}")]
    Http { target: String, status: u16, body: String },
    /// The body did not parse. A version skew between this worker and the agent image.
    #[error("agent at {target} returned an unreadable body: {message}")]
    Malformed { target: String, message: String },
    /// The agent did not become ready inside the deadline.
    #[error("agent at {target} was not ready within {seconds}s")]
    NotReady { target: String, seconds: u64 },
    /// The request could not be built or encoded, so nothing was sent.
    #[error("agent request for {target} could not be built: {message}")]
    Request { target: String, message: String },
    /// The agent refused this worker's protocol version, or speaks one older than this worker
    /// still drives. Retrying cannot help: the guest image or the worker must be upgraded, or
    /// the minimum lowered for the rollout (`SANDBOX_MIN_AGENT_PROTOCOL_VERSION` here,
    /// `SANDBOX_MIN_WORKER_PROTOCOL_VERSION` on the agent).
    #[error("agent at {target} is on an incompatible protocol: {reason}")]
    ProtocolMismatch { target: String, reason: GuestProtocolError },
}
