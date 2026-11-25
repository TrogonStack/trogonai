/// A point-in-time copy of a checkpoint task's counters.
///
/// A failure is an alert and never a readiness fault: the chain goes on, and the
/// next checkpoint covers a later head. A count that keeps rising means the
/// window a deleted tail can hide in keeps growing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckpointStats {
    pub(crate) written: u64,
    pub(crate) sign_failures: u64,
    pub(crate) publish_failures: u64,
}

impl CheckpointStats {
    /// Checkpoints the stream acknowledged.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Checkpoints the signer did not sign, or signed under a key that does not
    /// verify them.
    pub fn sign_failures(&self) -> u64 {
        self.sign_failures
    }

    /// Signed checkpoints the stream did not acknowledge.
    pub fn publish_failures(&self) -> u64 {
        self.publish_failures
    }
}
