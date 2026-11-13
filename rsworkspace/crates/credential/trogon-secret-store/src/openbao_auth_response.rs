use std::time::{Duration, Instant};

use serde::Deserialize;
use trogon_std::SecretString;

use super::{OpenBaoLease, OpenBaoLeaseRenewal};

/// OpenBao's answer to a login, a renewal, or a token creation.
///
/// The token is decoded straight into a [`SecretString`], so no plain copy of
/// it is ever held.
#[derive(Deserialize)]
pub(crate) struct OpenBaoAuthResponse {
    auth: OpenBaoAuthGrant,
}

#[derive(Deserialize)]
struct OpenBaoAuthGrant {
    client_token: SecretString,
    lease_duration: u64,
    renewable: bool,
}

impl OpenBaoAuthResponse {
    pub(crate) fn into_lease(self, obtained_at: Instant) -> OpenBaoLease {
        OpenBaoLease::new(
            self.auth.client_token,
            obtained_at,
            Duration::from_secs(self.auth.lease_duration),
            OpenBaoLeaseRenewal::from(self.auth.renewable),
        )
    }

    pub(crate) fn into_token(self) -> SecretString {
        self.auth.client_token
    }
}
