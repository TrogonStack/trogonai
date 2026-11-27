use std::io;

use trogon_sandbox_guest_init::GuestBootConfigError;

/// Why the guest init stopped instead of reaping children for the guest's lifetime.
#[derive(Debug, thiserror::Error)]
pub enum GuestInitError {
    #[error("trogon-sandbox-guest-init must run as guest PID 1")]
    NotGuestPidOne,
    #[error("guest agent exited with wait status {wait_status}")]
    AgentExited { wait_status: i32 },
    #[error(transparent)]
    BootConfig(#[from] GuestBootConfigError),
    #[error(transparent)]
    Io(#[from] io::Error),
}
