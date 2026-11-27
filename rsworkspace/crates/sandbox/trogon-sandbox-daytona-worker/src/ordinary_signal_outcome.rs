/// What a signal request to an ordinary command came to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrdinarySignalOutcome {
    /// The supervisor delivered the signal to the command's process family.
    Delivered,
    /// The command's process family was already gone.
    AlreadyCompleted,
}
