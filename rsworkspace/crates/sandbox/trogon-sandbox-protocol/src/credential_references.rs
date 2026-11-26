use std::collections::HashSet;

use trogon_sandbox_contract::{DomainRuleAction, NetworkPolicy, PullSecretId, SandboxSpec};

/// The NATS KV bucket the control plane commits each lease's credential policy to, and the credential broker and
/// workers read it from.
pub const COMMITTED_POLICY_BUCKET: &str = "sandbox-committed-credential-policies";

/// Every credential reference `spec` can ask the broker to resolve.
///
/// The control plane commits this set as the lease's credential policy, and the credential broker resolves only
/// references in it. Both a secret binding's source and a `Transform` domain rule's credential name one.
pub fn credential_references(spec: &SandboxSpec) -> HashSet<PullSecretId> {
    let mut references: HashSet<PullSecretId> = spec
        .secret_bindings()
        .bindings()
        .iter()
        .map(|binding| binding.source().pull_secret_id.clone())
        .collect();
    if let Some(NetworkPolicy::UserDefined { rules, .. }) = spec.network_policy() {
        for rule in rules {
            if let DomainRuleAction::Transform(transform) = &rule.action {
                references.insert(transform.credential.pull_secret_id.clone());
            }
        }
    }
    references
}
