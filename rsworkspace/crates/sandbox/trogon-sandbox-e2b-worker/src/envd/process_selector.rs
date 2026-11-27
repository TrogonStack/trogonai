use serde::Serialize;

/// `process.ProcessSelector`: a process is addressed by pid, or by the tag it was started
/// with, which is usable before the pid is known.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessSelector {
    Pid(u32),
    Tag(String),
}
