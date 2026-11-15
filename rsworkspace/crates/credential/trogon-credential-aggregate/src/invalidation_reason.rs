use trogonai_proto::gateway::credentials::CredentialEventCase;

/// Why a credential event stops a released value from being the one the
/// credential releases.
///
/// It is the aggregate's own reading of its events, so the read side decides
/// which events invalidate a release without depending on how any consumer
/// puts that on the wire.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InvalidationReason {
    /// A rotation completed, so a newer version releases instead.
    Rotated,
    /// The credential was revoked and releases nothing.
    Revoked,
    /// Destruction was requested or completed.
    Destroyed,
}

/// Whether `event` invalidates the values its credential released, and why.
///
/// An event invalidates when it ends the release of a version that
/// [`releasable_version`](crate::releasable_version) handed out before it. A
/// completed rotation ends the old version's release and a revocation ends all
/// of them. A requested rotation still releases the version it replaces and a
/// failed one keeps releasing it, so neither invalidates. Destruction only
/// follows a revoked or cleanup-failed credential, which already releases
/// nothing, so its request and its completion repeat the revocation for a
/// consumer that missed it rather than ending a release of their own; its
/// failure adds nothing its request did not say. A write, its failure, and an
/// activation come before anything released.
///
/// Every case is matched by name, so an event added to the stream does not
/// compile until it is classified here.
pub fn invalidation_reason(event: &CredentialEventCase) -> Option<InvalidationReason> {
    match event {
        CredentialEventCase::Rotated(_) => Some(InvalidationReason::Rotated),
        CredentialEventCase::Revoked(_) => Some(InvalidationReason::Revoked),
        CredentialEventCase::DestroyRequested(_) | CredentialEventCase::Destroyed(_) => {
            Some(InvalidationReason::Destroyed)
        }
        CredentialEventCase::WriteRequested(_)
        | CredentialEventCase::WriteFailed(_)
        | CredentialEventCase::Activated(_)
        | CredentialEventCase::RotationRequested(_)
        | CredentialEventCase::RotationFailed(_)
        | CredentialEventCase::DestroyFailed(_) => None,
    }
}

#[cfg(test)]
mod tests;
