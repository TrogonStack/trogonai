use serde::{Deserialize, Serialize};

use super::ActorClaimIdError;
use super::constants::{CLAIM_ID_ALPHABET, CLAIM_ID_LEN};

/// Identity of one worker's claim on an actor, a 26 character Crockford base32 ULID.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ActorClaimId(String);

impl ActorClaimId {
    /// A fresh, time ordered claim identity.
    pub fn generate() -> Self {
        Self(encode(uuid::Uuid::now_v7().as_u128()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn encode(mut value: u128) -> String {
    let mut out = [b'0'; CLAIM_ID_LEN];
    for slot in out.iter_mut().rev() {
        *slot = CLAIM_ID_ALPHABET[(value & 0x1f) as usize];
        value >>= 5;
    }
    out.iter().map(|byte| char::from(*byte)).collect()
}

fn decode(text: &str) -> Option<u128> {
    if text.len() != CLAIM_ID_LEN {
        return None;
    }
    let mut value: u128 = 0;
    for (index, byte) in text.bytes().enumerate() {
        let digit = CLAIM_ID_ALPHABET
            .iter()
            .position(|candidate| *candidate == byte.to_ascii_uppercase())?;
        // The leading character carries only 3 bits; more would overflow 128 bits.
        if index == 0 && digit > 7 {
            return None;
        }
        value = (value << 5) | digit as u128;
    }
    Some(value)
}

impl TryFrom<String> for ActorClaimId {
    type Error = ActorClaimIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        decode(&value).map(|id| Self(encode(id))).ok_or(ActorClaimIdError)
    }
}

impl From<ActorClaimId> for String {
    fn from(value: ActorClaimId) -> Self {
        value.0
    }
}
