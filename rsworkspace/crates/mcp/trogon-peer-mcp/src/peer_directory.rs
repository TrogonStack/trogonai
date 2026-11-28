use trogon_peer::{PeerDisplayName, PeerId};
use trogonai_proto::peers::v1alpha1 as v1;

use crate::constants::MAX_LISTED_PEERS;
use crate::recipient_list::RecipientList;
use crate::roster_page::RosterPage;

/// Display names last observed in the roster.
///
/// Names are peer-chosen labels used only to word guidance; they never select a recipient.
#[derive(Debug, Default)]
pub(crate) struct PeerDirectory(Vec<(PeerId, PeerDisplayName)>);

impl PeerDirectory {
    /// Merges a roster page; the whole roster replaces what was known, so departed peers drop out.
    pub(crate) fn observe(&mut self, response: &v1::RosterResponse, requested: RosterPage) -> RosterPage {
        let observed = match (requested, response.next_page_token.as_deref().is_none_or(str::is_empty)) {
            (RosterPage::Whole, true) => RosterPage::Whole,
            _ => RosterPage::Partial,
        };
        if observed == RosterPage::Whole {
            self.0.clear();
        }
        for peer in &response.peers {
            let (Some(Ok(id)), Some(Ok(name))) = (
                peer.peer_id.as_deref().map(PeerId::parse),
                peer.display_name.as_deref().map(PeerDisplayName::new),
            ) else {
                continue;
            };
            match self.0.iter_mut().find(|(known, _)| known == &id) {
                Some(entry) => entry.1 = name,
                None => self.0.push((id, name)),
            }
        }
        observed
    }

    pub(crate) fn name(&self, peer_id: &str) -> Option<&PeerDisplayName> {
        self.0
            .iter()
            .find(|(id, _)| id.as_str() == peer_id)
            .map(|(_, name)| name)
    }

    /// The peers other than `own` that a model can address.
    pub(crate) fn recipients(&self, own: &PeerId) -> RecipientList {
        RecipientList::new(
            self.0
                .iter()
                .filter(|(id, _)| id != own)
                .take(MAX_LISTED_PEERS)
                .cloned()
                .collect(),
        )
    }
}
