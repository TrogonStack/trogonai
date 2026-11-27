use crate::profile_grant_path_kind::ProfileGrantPathKind;

/// One path the grant wrote on, as `fence-helper` recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGrantPath {
    /// Canonical (`\\?\`-prefixed) path.
    pub path: String,
    pub kind: ProfileGrantPathKind,
}
