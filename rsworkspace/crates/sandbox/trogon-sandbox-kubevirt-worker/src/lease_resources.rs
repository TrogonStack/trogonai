use serde::{Deserialize, Serialize};

use crate::{ResourceReceipt, WarmCandidate, WarmClaimIntent};

/// Everything in the cluster that one lease owns or is about to own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseResources {
    pub vm: Option<ResourceReceipt>,
    pub data_volume: Option<ResourceReceipt>,
    pub pending_claim: Option<WarmClaimIntent>,
}

impl LeaseResources {
    /// The resources a lease owns once it claims `candidate`.
    pub fn claiming(candidate: &WarmCandidate) -> Self {
        Self {
            vm: Some(candidate.vm.clone()),
            data_volume: Some(candidate.data_volume.clone()),
            pending_claim: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vm.is_none() && self.data_volume.is_none() && self.pending_claim.is_none()
    }

    /// The key that names exactly these resources to the authority guard, or `None` when there are none.
    pub fn authority_key(&self) -> Option<String> {
        let claim = self.pending_claim.as_ref().map(|intent| &intent.candidate);
        let vm = self.vm.as_ref().or(claim.map(|candidate| &candidate.vm));
        let dv = self
            .data_volume
            .as_ref()
            .or(claim.map(|candidate| &candidate.data_volume));
        vm.or(dv).map(|primary| {
            format!(
                "{}/{}/{}/{}",
                primary.namespace,
                primary.name,
                vm.map_or("none", |receipt| receipt.uid.as_str()),
                dv.map_or("none", |receipt| receipt.uid.as_str())
            )
        })
    }
}
