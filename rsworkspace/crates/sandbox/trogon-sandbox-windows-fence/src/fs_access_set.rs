use serde::Serialize;

/// What `fence-helper acl grant` is handed: read and write grants, and the denies
/// stamped over them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsAccessSet {
    #[serde(rename = "read")]
    pub grant_read: Vec<String>,
    #[serde(rename = "write")]
    pub grant_write: Vec<String>,
    pub deny_read: Vec<String>,
    pub deny_write: Vec<String>,
}
