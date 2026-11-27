use serde::{Deserialize, Serialize};

/// How far one lease's allocation has come.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CustodyPhase {
    Preparing,
    Ready,
    Cleanup,
    Cleaned,
    Unresolved,
}
