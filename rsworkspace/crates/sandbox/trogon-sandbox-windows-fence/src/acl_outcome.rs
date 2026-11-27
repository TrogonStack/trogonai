use serde::Deserialize;

/// One path's outcome from `fence-helper acl revoke --json` or `acl restore
/// --json`. The status vocabulary differs between builds, so it stays text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AclOutcome {
    pub path: String,
    pub status: String,
}
