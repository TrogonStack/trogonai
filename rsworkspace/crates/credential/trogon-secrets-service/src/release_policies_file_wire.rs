use serde::Deserialize;

use super::release_policy_wire::ReleasePolicyWire;

/// The release policies file as written: one entry per credential.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleasePoliciesFileWire {
    pub(crate) policies: Vec<ReleasePolicyWire>,
}
