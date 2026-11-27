/// A restore the environment policy let through, and how.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RestoreVerdict {
    /// The snapshot's digest equals this worker's.
    Matched,
    /// The snapshot has no digest and the policy allows that. Callers should log a warning.
    LegacyUnlabeled,
    /// This worker computes no digest, so nothing is checked.
    Unchecked,
}
