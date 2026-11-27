use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{CommandOutputChunk, RunCommandResponse};

use crate::ExecBegin;

/// One line of the newline-delimited body the exec endpoint returns.
///
/// A streaming command opens with one [`ExecFrame::Begin`], emits zero or more [`ExecFrame::Chunk`] lines and always
/// terminates with exactly one [`ExecFrame::Response`]. The three shapes are disjoint, so an untagged enum reads the
/// stream without a framing header, and a reader that predates `Begin` drops it as an unreadable line.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecFrame {
    Chunk(CommandOutputChunk),
    Response(RunCommandResponse),
    Begin(ExecBegin),
}
