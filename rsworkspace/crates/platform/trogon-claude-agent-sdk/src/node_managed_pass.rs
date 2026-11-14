//! Which evaluation of the managed file tier (`pE` upstream) a read of managed settings documents models.
//!
//! `resolveSettings` evaluates the tier for the sources and the effective settings (two calls with the same room),
//! again for the policy origin (`IX`), then for the machine policy (`KX`), each on a stack with more room than the
//! one before. A document is parsed once per resolve, by the first evaluation that reaches it, so a later one only
//! parses the drop-ins the earlier ones never reached, after one gave up on its folder for want of stack.

/// An evaluation of the managed file tier, which sets the stack each of its passes has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedPass {
    /// The settings sources and the effective settings (`QP` through `$u`).
    Sources,
    /// The policy origin (`IX` through `$u`).
    Origin,
    /// The managed files the machine policy reports as not loaded (`KX`).
    MachinePolicy,
}
