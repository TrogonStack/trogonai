use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationReceipt {
    pub(crate) control_sequence: NonZeroU64,
    pub(crate) live_sequence: NonZeroU64,
}

impl PublicationReceipt {
    pub fn control_sequence(&self) -> NonZeroU64 {
        self.control_sequence
    }
    pub fn live_sequence(&self) -> NonZeroU64 {
        self.live_sequence
    }
}
