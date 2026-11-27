use serde::Deserialize;

/// Where the command is in its life, as the bridge reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OrdinaryPhase {
    Starting,
    Prepared,
    Held,
    Running,
    Draining,
    Absent,
    Failed,
}
