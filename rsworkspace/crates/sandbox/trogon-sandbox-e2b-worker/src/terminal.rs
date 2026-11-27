//! Interactive terminals backed by an envd PTY process.
//!
//! The hub is the one the SSH-backed workers share, in [`trogon_sandbox_ssh_gateway::TerminalHub`]; this module
//! adapts an envd PTY to its [`GuestShell`]. Each terminal is one `process.Process/Start` stream started with a
//! `pty` (the protocol is described in `crate::envd`, from E2B's `process.proto`):
//!
//! - output is the stream's `data.pty` events, copied into a bounded pipe the hub reads;
//! - input is the hub's writes, read from a bounded pipe and sent with `SendInput`;
//! - a resize is an `Update` call, and closing the terminal is a `SendSignal` SIGKILL;
//! - the exit status is the stream's `end` event.
//!
//! Each terminal carries a unique `tag`. The worker waits for the `start` event (the pid) before it hands the
//! terminal out, and addresses the process by pid from then on; the tag covers the one call made before the pid is
//! known, killing a process whose start never arrived. Because the hub also serves the SSH stream bridge, SSH
//! sessions ride the same PTYs. Behaviour is verified against an in-process fake envd only, not a live sandbox.

mod constants;
mod e2b_guest_shell;
mod pty_process;
mod pump_input;
mod pump_output;
mod pump_resize;
mod run_terminal_handler;
mod spawn_pty;
mod terminal_error;
#[cfg(test)]
mod tests;

pub use constants::TERMINAL_SUBJECT;
pub use e2b_guest_shell::E2bGuestShell;
pub use run_terminal_handler::run_terminal_handler;
pub use terminal_error::TerminalError;
pub use trogon_sandbox_ssh_gateway::{
    DEFAULT_TERMINAL_IDLE_TIMEOUT, GuestChannel, GuestError, GuestShell, HubSshHost as E2bSshHost,
    TerminalHub as E2bTerminals,
};
