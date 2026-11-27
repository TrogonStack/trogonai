//! Getting the tenant's source into the actor.
//!
//! Every other provider in this tree materializes a workspace on the worker host and then
//! pushes it into the sandbox. That is not available here: an actor's only inbound channel
//! is the agent's HTTP contract, and shipping a cloned tree through it would mean sending
//! every byte of the repository as a JSON array of numbers. The clone therefore runs
//! *inside* the actor, over the actor's own network, and the worker only sends the command.
//!
//! That makes `git` an image requirement. It already was one in spirit: the image must also
//! carry the agent binary at the path the actor template invokes, so "the image is built for
//! this provider" is an existing contract rather than a new one.

mod clone_url;
mod constants;
mod materialize;
mod redact;
mod workspace_error;

pub use constants::WORKSPACE_DIR;
pub use materialize::materialize;
pub use workspace_error::WorkspaceError;

#[cfg(test)]
mod tests;
