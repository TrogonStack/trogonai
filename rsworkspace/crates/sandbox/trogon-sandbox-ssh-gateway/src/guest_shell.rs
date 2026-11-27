use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, TerminalDimension};

use crate::{GuestChannel, GuestError};

/// What the terminals need from the guest: whether this worker holds a lease, a PTY channel into it, and a way to
/// forget the lease's connection. Workers implement it over SSH.
#[async_trait]
pub trait GuestShell: Send + Sync {
    /// Whether this worker holds `lease_id`.
    async fn holds(&self, lease_id: &LeaseId) -> bool;

    /// Opens a PTY channel of `size` (`cols`, `rows`) running `command`, or the user's login shell when it is `None`.
    async fn open(
        &self,
        lease_id: &LeaseId,
        size: (TerminalDimension, TerminalDimension),
        term: &str,
        command: Option<String>,
    ) -> Result<GuestChannel, GuestError>;

    /// Drops everything held for a released lease (connection, ephemeral key).
    async fn release(&self, lease_id: &LeaseId);

    /// Why `open` is refused for a lease this worker does not hold.
    fn not_held_reason(&self) -> &'static str {
        "the worker holds no sandbox for this lease"
    }
}
