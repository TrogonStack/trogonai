use serde::{Deserialize, Deserializer, Serialize, de};

use crate::digest_mismatch_wire::DigestMismatchWire;
use crate::{Digest, DigestMismatchError};

/// A restored snapshot whose digest differs from the one the lease asked for.
///
/// The two digests always differ; construction and deserialization both reject a pair that
/// matches.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize)]
pub struct DigestMismatch {
    expected: Digest,
    actual: Digest,
}

impl DigestMismatch {
    pub fn new(expected: Digest, actual: Digest) -> Result<Self, DigestMismatchError> {
        if expected == actual {
            return Err(DigestMismatchError::SameDigest);
        }
        Ok(Self { expected, actual })
    }

    pub fn expected(&self) -> &Digest {
        &self.expected
    }

    pub fn actual(&self) -> &Digest {
        &self.actual
    }
}

impl<'de> Deserialize<'de> for DigestMismatch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = DigestMismatchWire::deserialize(deserializer)?;
        Self::new(wire.expected, wire.actual).map_err(de::Error::custom)
    }
}
