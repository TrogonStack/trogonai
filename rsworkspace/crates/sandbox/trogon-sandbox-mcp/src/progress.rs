//! Live command output as MCP progress notifications.
//!
//! A client that sends a `progressToken` with `tools/call` can show a long command's output as it is produced,
//! instead of only the final result. The tool result stays what it always was; progress is purely additive.

mod follow_end;
mod progress_for_chunk;
mod pump_progress;

pub(crate) use follow_end::FollowEnd;
pub(crate) use pump_progress::pump_progress;
