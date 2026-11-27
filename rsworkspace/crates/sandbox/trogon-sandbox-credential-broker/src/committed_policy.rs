use std::collections::HashSet;

use trogon_sandbox_contract::PullSecretId;

/// What the control plane committed for one lease: the revision and the credential references that revision
/// names.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CommittedPolicy {
    pub revision: u64,
    pub references: HashSet<PullSecretId>,
}
