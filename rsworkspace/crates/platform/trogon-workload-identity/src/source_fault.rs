use crate::workload_response_error::WorkloadResponseError;
use crate::{JwtBundleError, MaterialError};

/// Why the Workload API source stopped supplying usable material. Each
/// variant has a stable reason code for logs; the codes are an operator
/// contract and must not be renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceFault {
    ConnectFailed,
    ConnectTimedOut,
    FirstResponseTimedOut,
    StreamEnded,
    StreamError,
    BoundsExceeded,
    CrlBearingResponse,
    IdentityMismatch,
    BundleConflict,
    InvalidJwtBundle,
    InvalidSvidProfile,
    InvalidCertificatePath,
    KeyMismatch,
    MaterialUnavailable,
    UnparsableResponse,
}

impl SourceFault {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::ConnectFailed => "connect_failed",
            Self::ConnectTimedOut => "connect_timed_out",
            Self::FirstResponseTimedOut => "first_response_timed_out",
            Self::StreamEnded => "stream_ended",
            Self::StreamError => "stream_error",
            Self::BoundsExceeded => "bounds_exceeded",
            Self::CrlBearingResponse => "crl_bearing_response",
            Self::IdentityMismatch => "identity_mismatch",
            Self::BundleConflict => "bundle_conflict",
            Self::InvalidJwtBundle => "invalid_jwt_bundle",
            Self::InvalidSvidProfile => "invalid_svid_profile",
            Self::InvalidCertificatePath => "invalid_certificate_path",
            Self::KeyMismatch => "key_mismatch",
            Self::MaterialUnavailable => "material_unavailable",
            Self::UnparsableResponse => "unparsable_response",
        }
    }
}

impl From<MaterialError> for SourceFault {
    fn from(error: MaterialError) -> Self {
        match error {
            MaterialError::Bounds => Self::BoundsExceeded,
            MaterialError::RevocationUnsupported => Self::CrlBearingResponse,
            MaterialError::Identity => Self::IdentityMismatch,
            MaterialError::Profile => Self::InvalidSvidProfile,
            MaterialError::Certificate => Self::InvalidCertificatePath,
            MaterialError::Key => Self::KeyMismatch,
            MaterialError::Unavailable => Self::MaterialUnavailable,
        }
    }
}

impl From<JwtBundleError> for SourceFault {
    fn from(error: JwtBundleError) -> Self {
        match error {
            JwtBundleError::TooLarge | JwtBundleError::TooManyAuthorities | JwtBundleError::TooManyBundles => {
                Self::BoundsExceeded
            }
            _ => Self::InvalidJwtBundle,
        }
    }
}

impl From<&WorkloadResponseError> for SourceFault {
    fn from(error: &WorkloadResponseError) -> Self {
        match error {
            WorkloadResponseError::Material(error) => (*error).into(),
            WorkloadResponseError::BundleConflict => Self::BundleConflict,
            WorkloadResponseError::Svid(_)
            | WorkloadResponseError::Bundle(_)
            | WorkloadResponseError::TrustDomain(_) => Self::UnparsableResponse,
        }
    }
}

#[cfg(test)]
mod tests;
