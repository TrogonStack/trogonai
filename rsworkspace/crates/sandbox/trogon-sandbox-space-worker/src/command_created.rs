use serde::Deserialize;

use crate::{Attach, SpaceId};

/// A created command and the data-plane credentials to read it.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandCreated {
    /// The process SPACE started or will start on attach.
    pub process_id: SpaceId,
    /// How to read the command's events.
    pub attach: Option<Attach>,
}
