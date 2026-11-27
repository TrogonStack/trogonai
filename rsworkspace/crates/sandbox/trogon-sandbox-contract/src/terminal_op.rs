use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{TerminalArgument, TerminalDimension, TerminalInput};

/// One operation on a terminal, tagged by `op`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum TerminalOp {
    /// Allocates a terminal of `cols` by `rows` and starts `cmd` on it, or the sandbox's default shell
    /// when `cmd` is empty. Opening an id that is already open on the same lease answers `opened` again.
    Open {
        cols: TerminalDimension,
        rows: TerminalDimension,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cmd: Vec<TerminalArgument>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Only meaningful when reopening an id that is already open: the highest output `seq` the
        /// caller has seen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_from: Option<u64>,
    },
    /// Writes bytes to the terminal's input. `data` is base64.
    Input { data: TerminalInput },
    /// Changes the terminal size.
    Resize {
        cols: TerminalDimension,
        rows: TerminalDimension,
    },
    /// Ends the terminal and releases it. Closing an unknown id answers `done`, so a retried close is
    /// safe.
    Close,
}
