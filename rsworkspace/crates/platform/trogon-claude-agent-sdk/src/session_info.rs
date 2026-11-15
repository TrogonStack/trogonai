use crate::SessionId;

/// Session metadata returned by the session listing helpers (`SDKSessionInfo` upstream).
///
/// `file_size` is only populated for local JSONL storage; store-backed listings leave it empty.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdkSessionInfo {
    pub session_id: SessionId,
    /// Display title: custom title, generated summary, or first prompt.
    pub summary: String,
    /// Last modified time in milliseconds since the Unix epoch.
    pub last_modified: u64,
    pub file_size: Option<u64>,
    pub custom_title: Option<String>,
    pub first_prompt: Option<String>,
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
    pub tag: Option<String>,
    /// Creation time in milliseconds since the Unix epoch, from the first entry timestamp.
    pub created_at: Option<i64>,
}
