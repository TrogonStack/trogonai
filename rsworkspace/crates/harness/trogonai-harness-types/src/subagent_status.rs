//! Live status facts for a managed subagent child.

use crate::config::ReasoningEffort;

/// What a subagent child was doing when its status was sampled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentStatus {
    /// The session title derived from the request, when there is one.
    pub session_title: Option<String>,
    /// The model the child is running.
    pub model: String,
    /// The reasoning effort in force.
    pub effort: ReasoningEffort,
    /// Input tokens consumed so far.
    pub input_tokens: u64,
    /// The model's context window, when known.
    pub context_window: Option<u32>,
}

/// Renders a subagent status line into a caller-provided buffer.
///
/// Not `Send + Sync`: renderers are borrowed for one publish on the thread
/// that owns the turn.
pub trait SubagentStatusRenderer {
    /// Formats `status` into `buf` and returns the slice written.
    fn render<'buf>(&self, buf: &'buf mut [u8], status: &SubagentStatus) -> &'buf str;
}
