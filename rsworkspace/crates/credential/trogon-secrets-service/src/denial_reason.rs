use std::fmt;

use trogon_credential_domain::RuntimeReleaseDeniedError;

use super::CredentialReleaseError;

/// Why a release was refused, for the audit record and the logs only.
///
/// The requester sees [`CredentialReleaseError::Denied`] for every reason except an
/// unusable xkey, so the reason never shapes the answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DenialReason {
    /// The requester xkey is not a curve public key.
    RequesterXkeyInvalid,
    /// No tenant is known by the importing account.
    UnknownTenant,
    /// The tenant does not hold the credential's owner.
    OwnerNotHeld,
    /// No release policy names the credential.
    NoReleasePolicy,
    /// The credential's release policy refuses this request.
    PolicyRefused(RuntimeReleaseDeniedError),
    /// The credential's stream holds no events.
    UnknownCredential,
    /// The credential's state releases no version.
    NotReleasable,
    /// The store no longer reads the releasable version, as when a revocation
    /// lands between the fold and the read.
    VersionUnreadable,
    /// The store holds only a verifier for the credential, never a value to
    /// send.
    VerifierOnly,
}

impl DenialReason {
    /// A stable name for the reason that audit queries and log filters key on.
    pub fn code(&self) -> &'static str {
        match self {
            Self::RequesterXkeyInvalid => "requester_xkey_invalid",
            Self::UnknownTenant => "unknown_tenant",
            Self::OwnerNotHeld => "owner_not_held",
            Self::NoReleasePolicy => "no_release_policy",
            Self::PolicyRefused(RuntimeReleaseDeniedError::RuntimeService { .. }) => "policy_refused_runtime_service",
            Self::PolicyRefused(RuntimeReleaseDeniedError::Host { .. }) => "policy_refused_host",
            Self::PolicyRefused(RuntimeReleaseDeniedError::InjectionLocation { .. }) => {
                "policy_refused_injection_location"
            }
            Self::UnknownCredential => "unknown_credential",
            Self::NotReleasable => "not_releasable",
            Self::VersionUnreadable => "version_unreadable",
            Self::VerifierOnly => "verifier_only",
        }
    }

    /// What the requester is told.
    pub fn failure(&self) -> CredentialReleaseError {
        match self {
            Self::RequesterXkeyInvalid => CredentialReleaseError::KeyDenied,
            Self::UnknownTenant
            | Self::OwnerNotHeld
            | Self::NoReleasePolicy
            | Self::PolicyRefused(_)
            | Self::UnknownCredential
            | Self::NotReleasable
            | Self::VersionUnreadable
            | Self::VerifierOnly => CredentialReleaseError::Denied,
        }
    }
}

impl fmt::Display for DenialReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PolicyRefused(error) => write!(f, "{}: {error}", self.code()),
            _ => f.write_str(self.code()),
        }
    }
}

#[cfg(test)]
mod tests;
