use crate::{constants::MAX_WORK, peer_port_error::PeerPortError};
#[derive(Debug, Clone, Copy)]
pub struct WorkLimit(usize);
impl WorkLimit {
    pub fn new(limit: usize) -> Result<Self, PeerPortError> {
        if !(1..=MAX_WORK).contains(&limit) {
            return Err(PeerPortError::Denied);
        }
        Ok(Self(limit))
    }
    pub fn get(self) -> usize {
        self.0
    }
}
