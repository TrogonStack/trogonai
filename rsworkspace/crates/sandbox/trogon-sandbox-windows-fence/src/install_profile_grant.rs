use crate::profile_grant_result::ProfileGrantResult;

/// What an install did about the profile read grant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum InstallProfileGrant {
    Applied {
        user_sid: String,
        result: ProfileGrantResult,
    },
    /// Sessions still run; they read nothing in the profile until the grant
    /// is applied.
    Skipped { reason: String },
}
