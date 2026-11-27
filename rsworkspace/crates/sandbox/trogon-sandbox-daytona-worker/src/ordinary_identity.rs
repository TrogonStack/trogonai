use serde::Deserialize;

/// A process the supervisor started, named by its id and the time it started, so a reused id is not mistaken for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrdinaryIdentity {
    pub(crate) pid: u32,
    pub(crate) start_time: u64,
}
