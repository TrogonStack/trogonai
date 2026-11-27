use std::collections::HashSet;

use serde::Serialize;

use crate::{AwsCredentialPairBinding, AwsCredentialPairsError, SecretBinding, SecretBindings};

/// AWS credential pairs checked against the bindings they link.
///
/// Every member exists exactly once among the bindings and belongs to one pair only. Every member is a whole-value
/// `mask` binding and injects at the same hosts and under the same scope as its pair's access key id, so the secret
/// is never swapped anywhere the access key id is not. Construction enforces these rules.
///
/// There is no `Deserialize`, because the rules need the bindings. Read the pairs as a list of
/// [`AwsCredentialPairBinding`] and build this with [`AwsCredentialPairs::new`]. It serializes as a JSON array.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AwsCredentialPairs(Vec<AwsCredentialPairBinding>);

impl AwsCredentialPairs {
    pub fn new(
        pairs: Vec<AwsCredentialPairBinding>,
        bindings: &SecretBindings,
    ) -> Result<Self, AwsCredentialPairsError> {
        let find = |name: &crate::SecretName| -> Result<&SecretBinding, AwsCredentialPairsError> {
            bindings
                .bindings()
                .iter()
                .find(|binding| binding.name() == name)
                .ok_or_else(|| AwsCredentialPairsError::UnknownBinding(name.clone()))
        };
        let mut used = HashSet::new();
        for pair in &pairs {
            let access_key_id = find(pair.access_key_id())?;
            for member in pair.members() {
                if !used.insert(member) {
                    return Err(AwsCredentialPairsError::DuplicateMember(member.clone()));
                }
                let binding = find(member)?;
                if !binding.is_whole_value_mask() {
                    return Err(AwsCredentialPairsError::MemberNotWholeMask(member.clone()));
                }
                let same_destination = binding.inject_hosts() == access_key_id.inject_hosts()
                    && binding.inject_scope() == access_key_id.inject_scope();
                if !same_destination {
                    return Err(AwsCredentialPairsError::HostsDiffer {
                        access_key_id: pair.access_key_id().clone(),
                        member: member.clone(),
                    });
                }
            }
        }
        Ok(Self(pairs))
    }

    pub fn pairs(&self) -> &[AwsCredentialPairBinding] {
        &self.0
    }
}
