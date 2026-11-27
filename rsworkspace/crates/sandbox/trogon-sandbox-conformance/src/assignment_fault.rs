/// Why a request could not be tied to the worker assignment the suite retained for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssignmentFaultError {
    /// The observed worker heartbeat and its profile name another provider than the target.
    #[error("the observed worker and profile must match the target provider")]
    ProviderMismatch,
    /// An assigned worker only accepts signed requests.
    #[error("assigned conformance requires a dispatch signer")]
    SignerRequired,
    /// A cleanup was requested with no assigned worker selected.
    #[error("cleanup has no selected assigned worker")]
    CleanupWithoutWorker,
    /// No assignment was retained for the lease.
    #[error("cleanup has no retained request assignment")]
    CleanupWithoutAssignment,
    /// The cleanup names another provider, assignment or subject than the retained one.
    #[error("cleanup differs from its retained request assignment")]
    CleanupDiffers,
    /// A per-lease request names a lease the suite never assigned.
    #[error("the request has no held conformance assignment")]
    RequestWithoutAssignment,
    /// The grant carries no evidence subject.
    #[error("the native grant is missing its evidence identity")]
    GrantMissingEvidence,
    /// The grant names another worker, attempt or epoch than the assignment.
    #[error("the native grant differs from the exact assigned worker/attempt/epoch")]
    GrantDiffersFromAssignment,
    /// The grant names another worker than the explicitly selected one.
    #[error("the native grant differs from the explicitly selected worker")]
    GrantDiffersFromSelectedWorker,
    /// An assigned worker returned a grant for a lease with no retained assignment.
    #[error("the native grant has no retained request assignment")]
    GrantWithoutAssignment,
    /// The cleanup reply did not confirm its exact assignment and release intent.
    #[error("cleanup did not confirm its exact assignment and release intent")]
    CleanupNotConfirmed,
}
