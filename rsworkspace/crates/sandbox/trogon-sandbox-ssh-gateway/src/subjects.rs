//! The NATS subjects of the SSH tunnel.

use trogon_sandbox_contract::LeaseId;

use crate::SshSessionId;
pub use crate::constants::{SSH_ATTACH_FILTER, SSH_DETACH_FILTER};

/// Where the gateway asks the worker holding `lease` to accept a session.
pub fn ssh_attach(lease: &LeaseId) -> String {
    format!("sandbox.ssh.attach.{lease}")
}

/// Where the gateway tells the worker holding `lease` that a session ended.
pub fn ssh_detach(lease: &LeaseId) -> String {
    format!("sandbox.ssh.detach.{lease}")
}

/// Frames from the gateway to the worker.
pub fn ssh_stream_to_worker(session: &SshSessionId) -> String {
    format!("sandbox.ssh.stream.{session}.to_worker")
}

/// Frames from the worker to the gateway.
pub fn ssh_stream_from_worker(session: &SshSessionId) -> String {
    format!("sandbox.ssh.stream.{session}.from_worker")
}
