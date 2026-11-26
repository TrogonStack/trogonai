use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::{
    Base64Data, FileOp, FileOpResponse, FileOverwrite, FileParentCreation, FilePresence, FileRecursion,
    FileResultTruncation, FileStat, LeaseId, SandboxUserName,
};

use crate::{ClientError, FileRange, SandboxClient, SearchOutcome, TransportError};

/// Decodes the payload of a file reply. The contract validated the encoding when it parsed the reply.
pub(crate) fn decode_data(operation: &'static str, data: &Base64Data) -> Result<Vec<u8>, ClientError> {
    STANDARD
        .decode(data.as_str())
        .map_err(|error| TransportError::InvalidRequest(format!("{operation}: {error}")).into())
}

impl SandboxClient {
    /// Lists a directory with stat information. The returned truncation says whether the listing hit
    /// `max_entries`.
    pub async fn list_dir(
        &self,
        lease_id: &LeaseId,
        path: &str,
        recursive: FileRecursion,
        max_entries: Option<u32>,
        as_user: Option<SandboxUserName>,
    ) -> Result<(Vec<FileStat>, FileResultTruncation), ClientError> {
        let op = FileOp::List {
            path: path.to_string(),
            recursive,
            max_entries,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Listing { entries, truncated } => Ok((entries, truncated)),
            other => Err(Self::unexpected_file_reply("list", other)),
        }
    }

    pub async fn stat(
        &self,
        lease_id: &LeaseId,
        path: &str,
        as_user: Option<SandboxUserName>,
    ) -> Result<FileStat, ClientError> {
        let op = FileOp::Stat { path: path.to_string() };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Stat { stat } => Ok(stat),
            other => Err(Self::unexpected_file_reply("stat", other)),
        }
    }

    /// Reads a byte range. The reply reports the file's total size and whether `max_bytes` cut the range short.
    pub async fn read_range(
        &self,
        lease_id: &LeaseId,
        path: &str,
        offset: u64,
        length: Option<u64>,
        max_bytes: Option<u64>,
        as_user: Option<SandboxUserName>,
    ) -> Result<FileRange, ClientError> {
        let op = FileOp::Read {
            path: path.to_string(),
            offset,
            length,
            max_bytes,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Content {
                data,
                offset,
                total_size,
                eof,
                truncated,
            } => Ok(FileRange {
                data: decode_data("read", &data)?,
                offset,
                total_size,
                eof,
                truncated,
            }),
            other => Err(Self::unexpected_file_reply("read", other)),
        }
    }

    /// Replaces every occurrence of `find` with `replace` when it occurs exactly `expected_occurrences` times;
    /// otherwise returns [`ClientError::EditRejected`] and the file is untouched. Returns the number of
    /// replacements.
    pub async fn edit_file(
        &self,
        lease_id: &LeaseId,
        path: &str,
        find: &str,
        replace: &str,
        expected_occurrences: u32,
        as_user: Option<SandboxUserName>,
    ) -> Result<u32, ClientError> {
        let op = FileOp::Edit {
            path: path.to_string(),
            find: find.to_string(),
            replace: replace.to_string(),
            expected_occurrences,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Edited { occurrences, .. } => Ok(occurrences),
            FileOpResponse::EditRejected { occurrences, expected } => {
                Err(ClientError::EditRejected { occurrences, expected })
            }
            other => Err(Self::unexpected_file_reply("edit", other)),
        }
    }

    /// Runs a [`FileOp::Search`]. Any other operation yields an unexpected-reply transport error.
    pub async fn search(
        &self,
        lease_id: &LeaseId,
        op: FileOp,
        as_user: Option<SandboxUserName>,
    ) -> Result<SearchOutcome, ClientError> {
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::SearchResults {
                matches,
                truncated,
                files_scanned,
            } => Ok(SearchOutcome {
                matches,
                truncated,
                files_scanned,
            }),
            other => Err(Self::unexpected_file_reply("search", other)),
        }
    }

    /// Removes a path. Returns whether it existed.
    pub async fn remove_path(
        &self,
        lease_id: &LeaseId,
        path: &str,
        recursive: FileRecursion,
        as_user: Option<SandboxUserName>,
    ) -> Result<FilePresence, ClientError> {
        let op = FileOp::Remove {
            path: path.to_string(),
            recursive,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Removed { existed } => Ok(existed),
            other => Err(Self::unexpected_file_reply("remove", other)),
        }
    }

    pub async fn mkdir(
        &self,
        lease_id: &LeaseId,
        path: &str,
        parents: FileParentCreation,
        as_user: Option<SandboxUserName>,
    ) -> Result<(), ClientError> {
        let op = FileOp::Mkdir {
            path: path.to_string(),
            parents,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Created { .. } => Ok(()),
            other => Err(Self::unexpected_file_reply("mkdir", other)),
        }
    }

    pub async fn rename_path(
        &self,
        lease_id: &LeaseId,
        from: &str,
        to: &str,
        overwrite: FileOverwrite,
        as_user: Option<SandboxUserName>,
    ) -> Result<(), ClientError> {
        let op = FileOp::Rename {
            from: from.to_string(),
            to: to.to_string(),
            overwrite,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Renamed => Ok(()),
            other => Err(Self::unexpected_file_reply("rename", other)),
        }
    }
}
