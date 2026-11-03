use crate::hold_name::HoldName;
use crate::hold_scope::HoldScope;
use crate::phase::Phase;

/// One hold that is currently placed on a subject.
///
/// Released holds are not part of a snapshot: the projector drops them, and a
/// released hold stops refusing within the staleness bound.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hold {
    name: HoldName,
    scope: HoldScope,
}

impl Hold {
    /// Describes a placed hold.
    pub fn new(name: HoldName, scope: HoldScope) -> Self {
        Self { name, scope }
    }

    /// The handle chosen by whoever placed the hold.
    pub fn name(&self) -> &HoldName {
        &self.name
    }

    /// How far the hold reaches.
    pub fn scope(&self) -> HoldScope {
        self.scope
    }

    /// Returns whether the hold refuses an action by a held subject's own
    /// Session, or by a Session in the lineage of the work, in this phase.
    pub fn refuses_in(&self, phase: Phase) -> bool {
        match (self.scope, phase) {
            (HoldScope::AllProtectedActions, _) => true,
            (HoldScope::NewWork, Phase::SessionAdmission) => true,
            (HoldScope::NewWork, Phase::ProtectedAction) => false,
        }
    }
}
