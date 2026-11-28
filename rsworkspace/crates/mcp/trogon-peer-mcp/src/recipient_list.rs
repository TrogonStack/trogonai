use std::fmt;

use trogon_peer::{PeerDisplayName, PeerId};

use crate::model_text::quoted;

/// Peers a model can address, as last observed in the roster, for error and reply guidance.
#[derive(Clone, Debug, Default)]
pub struct RecipientList(Vec<(PeerId, PeerDisplayName)>);

impl RecipientList {
    pub(crate) fn new(peers: Vec<(PeerId, PeerDisplayName)>) -> Self {
        Self(peers)
    }
}

impl fmt::Display for RecipientList {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return write!(formatter, "Call peer_roster to list the peers you can message.");
        }
        write!(formatter, "Peers you can message:")?;
        for (index, (id, name)) in self.0.iter().enumerate() {
            let separator = if index == 0 { " " } else { "; " };
            write!(
                formatter,
                "{separator}{} has recipient_peer_id {}",
                quoted(name.as_str()),
                id.as_str()
            )?;
        }
        write!(formatter, ".")
    }
}
