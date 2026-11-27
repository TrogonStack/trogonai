use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::{LiveOutput, MemoryFs};

/// Everything a scripted command can see.
pub struct CommandCtx {
    /// The arguments after the program name.
    pub args: Vec<String>,
    /// The working directory the command runs in.
    pub cwd: String,
    /// The environment the request carried.
    pub env: BTreeMap<String, String>,
    /// The lease's file map.
    pub fs: Arc<Mutex<MemoryFs>>,
    /// Cancelled when the command is killed or times out. Long-running
    /// handlers should stop when it fires.
    pub cancel: CancellationToken,
    /// Live output and listening ports, for commands that stay up.
    pub live: LiveOutput,
}
