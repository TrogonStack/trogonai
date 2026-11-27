use crate::StdinWrite;

/// What a [`StdinSequencer`](crate::StdinSequencer) decided about one admitted write.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum StdinAdmit {
    /// Apply these writes in order, then call [`StdinSequencer::commit`](crate::StdinSequencer::commit) with how
    /// many succeeded.
    Apply(Vec<StdinWrite>),
    /// The write was already applied.
    Duplicate,
    /// The write is held until the gap before it fills.
    Buffered,
    /// The write is too far ahead to hold. Resend from [`StdinSequencer::next_seq`](crate::StdinSequencer::next_seq).
    OutOfWindow,
    /// An earlier write closed stdin.
    Closed,
}
