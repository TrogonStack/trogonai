//! The X.509-SVID lifetime ceiling for certificates read from disk.
//!
//! The Workload API path bounds every SVID through
//! [`trogon_workload_identity`]. A static-file certificate never passes through
//! that path, so this module applies the same ceiling,
//! [`trogon_workload_identity::constants::MAX_SVID_LIFETIME`], to a peer
//! certificate in the handshake and to the proxy's own certificate at startup.
//! The ceiling is a constant, not a setting: nothing raises it.

use std::time::Duration;

use trogon_workload_identity::constants::MAX_SVID_LIFETIME;
use trogon_workload_identity::{MaterialError, leaf_lifetime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SvidLifetimeError {
    #[error("certificate validity period cannot be read")]
    Unreadable(#[source] MaterialError),
    #[error("certificate is valid for {lifetime:?}, longer than the {ceiling:?} X.509-SVID ceiling")]
    TooLong { lifetime: Duration, ceiling: Duration },
}

/// Refuses a DER leaf whose validity period (not-after minus not-before) is
/// zero, negative, unreadable or longer than one hour. It does not look at the
/// current time: chain validation owns expiry.
pub fn verify_svid_lifetime(certificate: &[u8]) -> Result<(), SvidLifetimeError> {
    let lifetime = leaf_lifetime(certificate).map_err(SvidLifetimeError::Unreadable)?;
    if lifetime > MAX_SVID_LIFETIME {
        return Err(SvidLifetimeError::TooLong {
            lifetime,
            ceiling: MAX_SVID_LIFETIME,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
