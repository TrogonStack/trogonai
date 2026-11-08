use crate::refusal::Refusal;

/// What the caller does with the request.
///
/// In `DryRun` a refusal still yields [`Effective::Admit`] here; the refusal is
/// kept in `Decision::would_deny`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effective {
    /// Let the request proceed.
    Admit,
    /// Stop the request.
    Refuse(Refusal),
}
