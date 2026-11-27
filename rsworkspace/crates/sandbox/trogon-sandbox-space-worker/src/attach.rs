use std::fmt;

use serde::Deserialize;

use crate::SpaceToken;

/// Where to read a command's events and the credential that opens them.
#[derive(Clone, Deserialize)]
pub struct Attach {
    /// The event stream URL, absolute or relative to the data plane.
    #[serde(default)]
    pub events_url: Option<String>,
    /// The per-command credential for the data plane.
    pub token: SpaceToken,
}

impl fmt::Debug for Attach {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attach")
            .field("events_url", &self.events_url)
            .field("token", &"<redacted>")
            .finish()
    }
}
