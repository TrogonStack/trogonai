use serde::{Deserialize, Serialize};

/// Whether a listed process is still running, tagged by `state`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ProcessState {
    Running,
    Exited { exit_code: i32 },
}
