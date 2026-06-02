//! CLI presentation layer.
//!
//! The unified CLI visual design lives here — stream rendering (`TurnRenderer`),
//! the permission prompt, and display helpers — so the REPL loop in `repl.rs`
//! stays focused on control flow. Add a `StreamEvent` match arm in
//! `turn_renderer.rs` when a new event variant lands.

mod display;
mod permission_prompt;
mod turn_renderer;

pub use display::{print_command_echo, print_startup_banner, print_user_line, runner_label};
pub use permission_prompt::{PermissionDisplay, permission_from_request, print_permission_prompt};
pub use turn_renderer::{CwdSync, TurnMetrics, TurnRenderer, TurnStop};
