use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LeaseAttemptIdentity;

use crate::{VmId, VmPhase};

/// What the worker remembers about one lease's VM. The journal keeps everything but the environment.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct LeaseRecord {
    pub(crate) vm: VmId,
    pub(crate) name: String,
    pub(crate) cwd: String,
    #[serde(skip)]
    pub(crate) env: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) network_slot: Option<usize>,
    #[serde(default)]
    pub(crate) phase: VmPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) assignment: Option<LeaseAttemptIdentity>,
}

impl LeaseRecord {
    /// Whether `other` describes the same VM allocation, whatever phase either has reached.
    pub(crate) fn same_allocation(&self, other: &Self) -> bool {
        self.vm == other.vm && self.name == other.name && self.assignment == other.assignment
    }
}
