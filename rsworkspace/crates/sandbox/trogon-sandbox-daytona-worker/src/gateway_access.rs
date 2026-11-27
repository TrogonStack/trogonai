use std::fmt;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::constants::{TOKEN_REFRESH_MARGIN, UNDATED_TOKEN_LIFETIME};

/// A gateway address and an access token for one sandbox.
///
/// `Debug` never shows the token.
#[derive(Clone)]
pub struct GatewayAccess {
    pub host: String,
    pub port: u16,
    /// The SSH user name, which is the access token.
    pub token: String,
    /// When the token stops working, in unix seconds.
    pub expires_at: Option<u64>,
}

impl GatewayAccess {
    /// Whether the token, minted at `minted`, is still good for a new login at `now` (`wall` is the same moment on the
    /// wall clock).
    pub(crate) fn usable(&self, minted: Instant, now: Instant, wall: SystemTime) -> bool {
        match self.expires_at {
            Some(at) => {
                let wall = wall.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
                wall.saturating_add(TOKEN_REFRESH_MARGIN.as_secs()) < at
            }
            None => now.saturating_duration_since(minted) < UNDATED_TOKEN_LIFETIME,
        }
    }
}

impl fmt::Debug for GatewayAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayAccess")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[cfg(test)]
mod tests;
