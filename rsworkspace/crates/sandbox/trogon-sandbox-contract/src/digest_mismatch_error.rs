/// A [`DigestMismatch`](crate::DigestMismatch) was given two equal digests.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum DigestMismatchError {
    #[error("a digest mismatch needs two different digests")]
    SameDigest,
}
