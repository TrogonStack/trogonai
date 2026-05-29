//! CLI presentation layer.
//!
//! Default behavior stays in `repl.rs`. Stream rendering and permission styling
//! live here so feature work on `programming-gaps` only needs a new `StreamEvent`
//! match arm when you tell us about a commit.

mod display;
mod permission_prompt;
mod turn_renderer;

pub use display::{print_startup_banner, print_user_line, runner_label};
pub use permission_prompt::{PermissionDisplay, permission_from_request, print_permission_prompt};
pub use turn_renderer::{CwdSync, TurnMetrics, TurnRenderer, TurnStop};
