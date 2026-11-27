use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SandboxFileWriteInput {
    pub path: String,
    pub content_base64: String,
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WriteFilesInput {
    pub lease_id: String,
    pub files: Vec<SandboxFileWriteInput>,
    /// Writes the files as this sandbox user instead of the lease's default user.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReadFileInput {
    pub lease_id: String,
    pub path: String,
    /// Reads the file as this sandbox user instead of the lease's default user.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReadFileOutput {
    pub content_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct FilePathInput {
    pub lease_id: String,
    pub path: String,
    /// Runs the operation as this sandbox user instead of the lease's default user.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ListDirInput {
    pub lease_id: String,
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
    /// Caps the number of entries returned; the reply reports `truncated` when it bites.
    #[serde(default)]
    pub max_entries: Option<u32>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReadRangeInput {
    pub lease_id: String,
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    /// Bytes to read; omitted reads to the end of the file, bounded by `max_bytes`.
    #[serde(default)]
    pub length: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct EditFileInput {
    pub lease_id: String,
    pub path: String,
    /// Exact text to replace. It must be non-empty.
    pub find: String,
    pub replace: String,
    /// The edit applies only when `find` occurs exactly this many times (default 1).
    #[serde(default)]
    pub expected_occurrences: Option<u32>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SearchFilesInput {
    pub lease_id: String,
    pub path: String,
    /// Filters file paths relative to `path` (`*`, `**`, `?`).
    #[serde(default)]
    pub glob: Option<String>,
    /// Greps file content line by line.
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_insensitive: bool,
    #[serde(default)]
    pub max_results: Option<u32>,
    #[serde(default)]
    pub max_file_bytes: Option<u64>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RemovePathInput {
    pub lease_id: String,
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct MkdirInput {
    pub lease_id: String,
    pub path: String,
    /// Creates missing parents (default true).
    #[serde(default)]
    pub parents: Option<bool>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RenamePathInput {
    pub lease_id: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ArchiveGetInput {
    pub lease_id: String,
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct CheckpointInput {
    pub lease_id: String,
    /// Root of the tree to record as the baseline, for example `/workspace`.
    pub path: String,
    /// Also fingerprint file content, so a rewrite with identical bytes is not a change.
    #[serde(default)]
    pub hash: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ChangesInput {
    pub lease_id: String,
    /// Checkpoint id from sandbox_checkpoint; omit to compare against the lease start.
    #[serde(default)]
    pub since: Option<String>,
    /// Narrow the report to one subtree of the baseline.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WatchStartInput {
    pub lease_id: String,
    /// File or directory to watch, for example `/workspace`.
    pub path: String,
    /// Also report changes in subdirectories (default false).
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WatchPollInput {
    pub lease_id: String,
    /// Id returned by sandbox_watch_start.
    pub watch_id: String,
    /// Return only events with a larger `seq`. Pass the `next_seq` of the previous poll.
    #[serde(default)]
    pub after_seq: u64,
    /// Longest wait for a new event when none is pending, in milliseconds. Capped at 20000;
    /// omit or pass 0 to return at once.
    #[serde(default)]
    pub wait_ms: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WatchStopInput {
    pub lease_id: String,
    pub watch_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ArchivePutInput {
    pub lease_id: String,
    /// Directory the tar stream is unpacked into; created when absent.
    pub path: String,
    pub archive_base64: String,
    #[serde(default)]
    pub user: Option<String>,
}
