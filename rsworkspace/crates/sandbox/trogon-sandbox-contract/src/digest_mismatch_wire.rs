use serde::Deserialize;

use crate::Digest;

/// The JSON shape of a [`DigestMismatch`](crate::DigestMismatch) before the digests are compared.
#[derive(Deserialize)]
pub(crate) struct DigestMismatchWire {
    pub(crate) expected: Digest,
    pub(crate) actual: Digest,
}
