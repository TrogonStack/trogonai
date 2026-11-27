use serde::{Deserialize, Serialize};

/// What the worker knows about one snapshot it asked Daytona to take.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Capture {
    /// The snapshot's id, once Daytona has reported it.
    pub(super) id: Option<String>,
    /// The capture was handed to the caller, so cleanup of the source must leave it alone.
    pub(super) kept: bool,
    /// The snapshot was deleted and its absence confirmed.
    pub(super) deleted: bool,
    /// The name was confirmed free before the capture was asked for.
    #[serde(default)]
    pub(super) pre_create_absent: bool,
    /// Daytona accepted the capture request.
    #[serde(default)]
    pub(super) create_acknowledged: bool,
}

impl Capture {
    /// A capture whose name was confirmed free and which Daytona has not yet been asked to take.
    pub(super) fn intent() -> Self {
        Self {
            id: None,
            kept: false,
            deleted: false,
            pre_create_absent: true,
            create_acknowledged: false,
        }
    }
}
