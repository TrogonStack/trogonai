use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{TerminalOutput, TerminalResponse};

/// One line of the body the terminal open endpoint returns.
///
/// The first line is always a [`TerminalResponse`]: `Opened`, or the reason the terminal could not start (the stream
/// then ends). After `Opened` come [`TerminalOutput`] frames with `seq` starting at 0 and no gaps, ending with exactly
/// one `Exit`. The two shapes are disjoint, so an untagged enum reads the stream.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TerminalFrame {
    Output(TerminalOutput),
    Response(TerminalResponse),
}
