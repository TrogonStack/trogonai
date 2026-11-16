use std::time::{Duration, Instant};

use trogon_std::SecretString;

use super::constants::{RENEWAL_WINDOW_DIVISOR, TOKEN_EXPIRY_MARGIN};

/// Whether OpenBao lets a login token extend its own lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenBaoLeaseRenewal {
    Renewable,
    Fixed,
}

impl From<bool> for OpenBaoLeaseRenewal {
    fn from(renewable: bool) -> Self {
        if renewable { Self::Renewable } else { Self::Fixed }
    }
}

/// A login token and the time it is good for.
///
/// `obtained_at` is taken before the call that produced the token was sent, so
/// the lease never looks longer than OpenBao granted.
pub(crate) struct OpenBaoLease {
    token: SecretString,
    obtained_at: Instant,
    ttl: Duration,
    renewal: OpenBaoLeaseRenewal,
}

impl OpenBaoLease {
    pub(crate) fn new(token: SecretString, obtained_at: Instant, ttl: Duration, renewal: OpenBaoLeaseRenewal) -> Self {
        Self {
            token,
            obtained_at,
            ttl,
            renewal,
        }
    }

    pub(crate) fn token(&self) -> &SecretString {
        &self.token
    }

    pub(crate) fn ttl(&self) -> Duration {
        self.ttl
    }

    pub(crate) fn remaining(&self, now: Instant) -> Duration {
        self.ttl.saturating_sub(now.saturating_duration_since(self.obtained_at))
    }

    /// A token this close to expiry is never sent: a read made with it could
    /// outlive it.
    pub(crate) fn is_usable(&self, now: Instant) -> bool {
        self.remaining(now) > TOKEN_EXPIRY_MARGIN
    }

    pub(crate) fn can_renew(&self, now: Instant) -> bool {
        self.renewal == OpenBaoLeaseRenewal::Renewable && self.is_usable(now)
    }

    /// Renewal starts in the last part of the lease, and never later than the
    /// point where the token stops being usable.
    pub(crate) fn is_renewal_due(&self, now: Instant) -> bool {
        let window = (self.ttl / RENEWAL_WINDOW_DIVISOR).max(TOKEN_EXPIRY_MARGIN);
        self.remaining(now) <= window
    }
}

#[cfg(test)]
mod tests;
