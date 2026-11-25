use serde::{Deserialize, Serialize};

/// How a worker receives lease dispatches.
///
/// A `legacy` worker takes dispatches from its provider's shared subject without an assignment. A
/// `targeted_v1` worker takes dispatches addressed to its own worker id, each carrying a dispatch
/// attempt. A heartbeat that omits the field reads as `legacy`.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerDispatchProtocol {
    #[default]
    Legacy,
    TargetedV1,
}
