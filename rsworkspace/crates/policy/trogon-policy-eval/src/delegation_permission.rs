use std::collections::BTreeSet;

use crate::callee::Callee;
use crate::delegation_target::DelegationTarget;

/// The targets whose Sessions a subject may dispatch.
///
/// This is the grant. A `DelegateDeclaration` grants nothing, so a subject with
/// no permission on its chain may not delegate (ADR#0071 Decision 6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DelegationPermission {
    targets: BTreeSet<DelegationTarget>,
}

impl DelegationPermission {
    /// Builds a permission from the targets it names.
    pub fn new(targets: impl IntoIterator<Item = DelegationTarget>) -> Self {
        Self {
            targets: targets.into_iter().collect(),
        }
    }

    /// Returns whether any target covers the resolved callee.
    pub fn covers(&self, callee: &Callee) -> bool {
        self.targets.iter().any(|target| target.covers(callee))
    }

    /// Iterates the targets in sorted order.
    pub fn targets(&self) -> impl Iterator<Item = &DelegationTarget> {
        self.targets.iter()
    }
}

impl FromIterator<DelegationTarget> for DelegationPermission {
    fn from_iter<I: IntoIterator<Item = DelegationTarget>>(targets: I) -> Self {
        Self::new(targets)
    }
}
