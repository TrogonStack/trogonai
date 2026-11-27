use serde::{Deserialize, Serialize};

/// How far a VM's creation or destruction has got, as the journal remembers it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum VmPhase {
    #[default]
    Unknown,
    CloneTask {
        upid: String,
    },
    DestroyTask {
        upid: String,
    },
    Ready,
    Destroyed,
}
