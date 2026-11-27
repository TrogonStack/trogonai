use serde::{Deserialize, Serialize};

use crate::{
    Base64Data, FileCaseSensitivity, FileContentHashing, FileOverwrite, FileParentCreation, FileRecursion,
    FileSearchSyntax, FileWriteTruncation,
};

fn one() -> u32 {
    1
}

/// One structured file operation, tagged by `op`.
///
/// Every operation travels through one routed verb, so a worker that cannot serve files can
/// answer a single subject with an unsupported reply. Payload bytes are base64 strings.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum FileOp {
    /// Lists a directory with stat information for every entry.
    List {
        path: String,
        #[serde(default)]
        recursive: FileRecursion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_entries: Option<u32>,
    },
    Stat {
        path: String,
    },
    /// Reads a byte range. A `length` of `None` reads to the end, bounded by `max_bytes`. The
    /// reply reports the total size and whether the range was cut short.
    Read {
        path: String,
        #[serde(default)]
        offset: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },
    /// Writes `data` at `offset`. `truncate` empties the file first, so a chunked upload sends
    /// its first chunk with `truncate` and the rest at increasing offsets.
    Write {
        path: String,
        data: Base64Data,
        #[serde(default)]
        offset: u64,
        #[serde(default)]
        truncate: FileWriteTruncation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<u32>,
        #[serde(default)]
        create_parents: FileParentCreation,
    },
    /// Replaces every occurrence of `find` with `replace` when `find` occurs exactly
    /// `expected_occurrences` times. Any other count leaves the file untouched.
    Edit {
        path: String,
        find: String,
        replace: String,
        #[serde(default = "one")]
        expected_occurrences: u32,
    },
    /// Searches under `path`. `glob` filters file paths relative to `path`, and `pattern`
    /// additionally greps file content line by line.
    Search {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glob: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        /// Whether `pattern` is a regular expression instead of a literal string.
        #[serde(default)]
        regex: FileSearchSyntax,
        #[serde(default)]
        case_insensitive: FileCaseSensitivity,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_results: Option<u32>,
        /// Files larger than this are skipped when grepping.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_file_bytes: Option<u64>,
    },
    Remove {
        path: String,
        #[serde(default)]
        recursive: FileRecursion,
    },
    Mkdir {
        path: String,
        #[serde(default)]
        parents: FileParentCreation,
    },
    Rename {
        from: String,
        to: String,
        #[serde(default)]
        overwrite: FileOverwrite,
    },
    /// Packs a file or directory tree into one tar stream.
    ArchiveGet {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },
    /// Unpacks a tar stream into the directory `path`, creating it when absent.
    ArchivePut {
        path: String,
        data: Base64Data,
    },
    /// Records the state of the tree under `path` as a baseline for a later changes request.
    /// With content hashing a rewrite with identical bytes is not a change. Without it a change
    /// is a different size or modification time.
    Checkpoint {
        path: String,
        #[serde(default)]
        hash: FileContentHashing,
    },
    /// Reports the paths added, modified, and deleted since a baseline: the checkpoint named by
    /// `since`, or the lease start when `since` is absent. `path` narrows the report to one
    /// subtree and defaults to the baseline's whole tree.
    Changes {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// Starts watching `path` and answers with a watch id to poll for events.
    WatchStart {
        path: String,
        #[serde(default)]
        recursive: FileRecursion,
    },
    /// Returns the events of a watch with a sequence number above `after_seq`. When there are
    /// none it parks for up to `wait_ms` for the next one.
    WatchPoll {
        watch_id: String,
        #[serde(default)]
        after_seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u32>,
    },
    WatchStop {
        watch_id: String,
    },
    /// SHA-256 of the byte range `[offset, offset + length)` of a file. A `length` of `None`
    /// hashes to the end.
    Checksum {
        path: String,
        #[serde(default)]
        offset: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length: Option<u64>,
    },
}

impl FileOp {
    /// Largest raw byte count one read reply carries, and the ceiling of `max_bytes`.
    pub const MAX_READ_BYTES: u64 = 16 * 1024 * 1024;
    /// Largest raw byte count one write request may carry.
    pub const MAX_WRITE_BYTES: u64 = 16 * 1024 * 1024;
    /// Largest tar stream, in raw bytes, one archive get or put may carry.
    pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
    /// Entries one listing or search returns when the request names no cap.
    pub const DEFAULT_MAX_ENTRIES: u32 = 1000;
    /// Hard cap on entries one listing or search returns.
    pub const MAX_ENTRIES: u32 = 20_000;
    /// Largest file a content search opens when the request names no smaller one, in bytes.
    pub const DEFAULT_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;
    /// Longest a watch poll parks waiting for the next event, in milliseconds.
    pub const WATCH_MAX_WAIT_MS: u32 = 20_000;
    /// Most events one watch poll reply carries.
    pub const WATCH_MAX_EVENTS_PER_POLL: usize = 1000;
    /// Most entries a tracking manifest, such as a checkpoint or watch snapshot, may hold.
    pub const TRACKING_MAX_ENTRIES: usize = 100_000;
}
