use crate::{MaskedClaim, Sentinel};

/// The fake token for a real one and the sentinel standing in for each masked claim in it.
pub struct MaskedJwt {
    pub fake: Sentinel,
    pub claims: Vec<MaskedClaim>,
}
