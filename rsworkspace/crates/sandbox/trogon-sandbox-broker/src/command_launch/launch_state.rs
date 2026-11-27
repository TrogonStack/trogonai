use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum LaunchState {
    /// Dispatched to the worker; no reply recorded yet.
    Pending,
    /// The worker accepted the launch with this reply.
    Done { response: RunCommandResponse },
    /// The reply was delivered and its replay window has passed. The key
    /// still blocks a second run, but the output is no longer kept.
    Retired,
}
