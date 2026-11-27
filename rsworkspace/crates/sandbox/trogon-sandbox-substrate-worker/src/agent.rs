//! The worker's side of the in-actor agent's HTTP contract.
//!
//! `ateapi.Control` has no exec RPC, so this is the only way a command reaches a sandbox.
//! The router does not route by hostname: it selects an actor purely by the
//! `ate-target-actor` header on an HTTP CONNECT request, so every call here connects to the
//! router, authenticates TLS when configured, tunnels with that header, and only then speaks the
//! agent's plain HTTP/1.1 contract over the tunnel. This is also what makes exec work
//! against a suspended actor: the router resumes the actor on the CONNECT rather than
//! failing it.
//!
//! Nothing here decides policy. The agent already owns validation, detached execution,
//! signal delivery and lease-mismatch refusal, so a `Denied` coming back over HTTP is a
//! verdict to forward, not an error to translate.

mod actor_agent;
mod actor_target;
mod agent_error;
mod chunk_sink;
mod connect_response_error;
mod constants;
mod discard_chunks;
mod read_connect_response;

pub use actor_agent::ActorAgent;
pub use actor_target::ActorTarget;
pub use agent_error::AgentError;
pub use chunk_sink::ChunkSink;
pub use discard_chunks::DiscardChunks;

#[cfg(test)]
pub(crate) mod tests;
