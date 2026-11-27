use trogon_sandbox_contract::ClaimName;

use crate::{RealValue, Sentinel};

/// A masked top-level claim: the sentinel standing in for it and the real value it replaced.
pub struct MaskedClaim {
    pub claim: ClaimName,
    pub sentinel: Sentinel,
    pub real: RealValue,
}
