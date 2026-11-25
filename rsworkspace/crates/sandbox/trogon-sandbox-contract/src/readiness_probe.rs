use std::num::NonZeroU16;

use serde::{Deserialize, Serialize};

use crate::ReadinessNeedle;

/// What makes a running process ready, tagged by `kind`.
///
/// A port of 0 is rejected on deserialization.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ReadinessProbe {
    /// The process, inside the sandbox, accepts TCP connections on `port`.
    TcpPort { port: NonZeroU16 },
    /// `needle` appears in the process's stdout or stderr.
    OutputContains { needle: ReadinessNeedle },
}
