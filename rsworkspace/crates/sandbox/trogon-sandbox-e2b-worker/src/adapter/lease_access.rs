/// What a held lease still lets the worker do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeaseAccess {
    Ready,
    /// The lease only awaits cleanup; no verb may reach its sandbox.
    CleanupOnly,
}
