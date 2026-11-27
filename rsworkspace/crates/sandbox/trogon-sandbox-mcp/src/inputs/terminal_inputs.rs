use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TerminalOpenInput {
    pub lease_id: String,
    /// Id for the terminal: 1 to 64 letters, digits, '-' or '_'. Omit to get a fresh one.
    #[serde(default)]
    pub terminal_id: Option<String>,
    /// Width in columns (default 120).
    #[serde(default)]
    pub cols: Option<u16>,
    /// Height in rows (default 40).
    #[serde(default)]
    pub rows: Option<u16>,
    /// Program and arguments to run on the terminal. Omit for the sandbox's default shell.
    #[serde(default)]
    pub command: Vec<String>,
    /// Extra environment variables for the program.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Directory to start in. Omit for the sandbox's working directory.
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TerminalOpenOutput {
    pub terminal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TerminalWriteInput {
    pub lease_id: String,
    pub terminal_id: String,
    /// Text to type. A terminal takes "\r" as Enter and "\u0003" as Ctrl-C.
    #[serde(default)]
    pub text: Option<String>,
    /// Raw bytes to type, base64. Set this or `text`, not both.
    #[serde(default)]
    pub data_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TerminalReadInput {
    pub lease_id: String,
    pub terminal_id: String,
    /// Longest wait for output when none is buffered, in milliseconds. Capped at 20000; omit or pass 0 to return at
    /// once.
    #[serde(default)]
    pub wait_ms: Option<u32>,
    /// Most bytes to return (default 65536). The rest stays buffered for the next read.
    #[serde(default)]
    pub max_bytes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TerminalReadOutput {
    /// What the terminal printed since the last read, escape sequences included.
    pub output: String,
    /// Bytes lost because more than 256 KiB went unread.
    pub dropped_bytes: u64,
    /// Output frames that never reached this server.
    pub missed_frames: u64,
    /// The terminal ended and all of its output has been read.
    pub exited: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TerminalResizeInput {
    pub lease_id: String,
    pub terminal_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TerminalCloseInput {
    pub lease_id: String,
    pub terminal_id: String,
}
