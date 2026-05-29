//! CLI presentation layer.
//!
//! Stream rendering, the permission prompt, and display helpers live here so the
//! REPL loop in `repl.rs` stays focused on control flow. Behavior mirrors the
//! REPL's original inline rendering; add a `StreamEvent` match arm in
//! `turn_renderer.rs` when a new event variant lands.

mod display;
mod permission_prompt;
mod turn_renderer;

pub use display::{print_startup_banner, print_user_line};
pub use permission_prompt::{permission_summary, print_permission_prompt};
pub use turn_renderer::{CwdSync, TurnMetrics, TurnRenderer, TurnStop};
