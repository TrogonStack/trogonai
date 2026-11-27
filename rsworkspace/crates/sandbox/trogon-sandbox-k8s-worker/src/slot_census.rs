use std::collections::HashMap;

use trogon_sandbox_contract::Digest;

use crate::WarmSlotState;

/// Idle slots per digest, by state.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct SlotCensus {
    pub(crate) ready: HashMap<Digest, u32>,
    pub(crate) warming: HashMap<Digest, u32>,
}

impl SlotCensus {
    pub(crate) fn add(&mut self, state: Option<WarmSlotState>, digest: Option<Digest>) {
        let Some(digest) = digest else { return };
        match state {
            Some(WarmSlotState::Ready) => *self.ready.entry(digest).or_insert(0) += 1,
            Some(WarmSlotState::Warming) => *self.warming.entry(digest).or_insert(0) += 1,
            _ => {}
        }
    }
}
