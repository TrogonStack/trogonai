use serde::{Deserialize, Serialize};

use crate::{
    Base64Data, FileOpErrorKind, FilePresence, FileReadEnd, FileResultTruncation, FileSha256, FileStat, FileWatchEvent,
    FileWatchOverflow, LeaseError, SearchMatch,
};

/// A worker's answer to a file operation request, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum FileOpResponse {
    Listing {
        entries: Vec<FileStat>,
        truncated: FileResultTruncation,
    },
    Stat {
        stat: FileStat,
    },
    Content {
        data: Base64Data,
        offset: u64,
        total_size: u64,
        /// Whether `data` ends at the end of the file.
        eof: FileReadEnd,
        /// Whether the range was shortened by `max_bytes` or the reply ceiling.
        truncated: FileResultTruncation,
    },
    Written {
        bytes: u64,
        size: u64,
    },
    Edited {
        occurrences: u32,
        size: u64,
    },
    /// The file holds a different number of occurrences than the edit expected, and nothing was
    /// written.
    EditRejected {
        occurrences: u32,
        expected: u32,
    },
    SearchResults {
        matches: Vec<SearchMatch>,
        truncated: FileResultTruncation,
        files_scanned: u64,
    },
    Removed {
        existed: FilePresence,
    },
    Created {
        already_existed: FilePresence,
    },
    Renamed,
    Archive {
        data: Base64Data,
        entries: u64,
        bytes: u64,
    },
    Extracted {
        entries: u64,
        bytes: u64,
    },
    Checkpointed {
        checkpoint_id: String,
        /// Entries, files and directories, the baseline holds.
        entries: u64,
    },
    /// Net difference between the baseline and now. Paths are as the sandbox addresses them.
    Changes {
        /// The baseline compared against: a checkpoint id or [`FileOpResponse::CHANGES_SINCE_START`].
        since: String,
        added: Vec<String>,
        modified: Vec<String>,
        deleted: Vec<String>,
        /// Whether the lists were cut at the entry ceiling.
        truncated: FileResultTruncation,
    },
    WatchStarted {
        watch_id: String,
    },
    WatchEvents {
        events: Vec<FileWatchEvent>,
        /// The `after_seq` for the following poll.
        next_seq: u64,
        /// Whether older events fell out of the watch buffer before they were polled.
        overflowed: FileWatchOverflow,
    },
    WatchStopped {
        existed: FilePresence,
    },
    /// Checksum of the requested range. `bytes` is the length hashed and `total_size` is the
    /// whole file's size.
    Checksum {
        sha256: FileSha256,
        bytes: u64,
        total_size: u64,
    },
    NotFound {
        path: String,
    },
    Failed {
        kind: FileOpErrorKind,
        message: String,
    },
    /// The provider does not serve this file operation.
    Unsupported {
        message: String,
    },
    Denied(LeaseError),
}

impl FileOpResponse {
    /// The `since` value of a changes reply whose baseline is the lease start.
    pub const CHANGES_SINCE_START: &'static str = "start";
}
