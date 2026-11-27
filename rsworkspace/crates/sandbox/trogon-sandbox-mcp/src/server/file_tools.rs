use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};
use trogon_sandbox_contract::{
    Base64Data, FileCaseSensitivity, FileContentHashing, FileOp, FileOverwrite, FileParentCreation, FileRecursion,
    FileSearchSyntax, SandboxFileWrite,
};

use super::SandboxMcpServer;
use crate::ack::Ack;
use crate::inputs::file_inputs::{
    ArchiveGetInput, ArchivePutInput, ChangesInput, CheckpointInput, EditFileInput, FilePathInput, ListDirInput,
    MkdirInput, ReadFileInput, ReadFileOutput, ReadRangeInput, RemovePathInput, RenamePathInput, SearchFilesInput,
    WatchPollInput, WatchStartInput, WatchStopInput, WriteFilesInput,
};
use crate::parse;
use crate::reply::JsonReply;
use crate::tool_error::ToolError;

/// A watch poll never waits longer than the worker would.
pub(super) fn bounded_watch_wait(wait_ms: Option<u32>) -> Option<u32> {
    wait_ms.map(|ms| ms.min(FileOp::WATCH_MAX_WAIT_MS))
}

#[tool_router(router = file_router, vis = "pub(super)")]
impl SandboxMcpServer {
    #[tool(
        description = "Write one or more files into a running lease's sandbox. Mirrors trogon_sandbox_contract::WriteFilesRequest."
    )]
    async fn sandbox_write_files(&self, params: Parameters<WriteFilesInput>) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let as_user = parse::optional_username(params.0.user)?;
        let mut files = Vec::new();
        for file in params.0.files {
            let content = STANDARD
                .decode(file.content_base64)
                .map_err(|e| ToolError::invalid("content_base64", e))?;
            let path = self.confine_path(&lease_id, &as_user, &file.path).await?;
            files.push(SandboxFileWrite {
                path,
                content,
                mode: file.mode,
            });
        }
        self.client()?.write_files(&lease_id, files, as_user).await?;
        Ok(Ack::reply())
    }

    #[tool(
        description = "Read a single file out of a running lease's sandbox. Mirrors trogon_sandbox_contract::ReadFileRequest."
    )]
    async fn sandbox_read_file(&self, params: Parameters<ReadFileInput>) -> Result<Json<ReadFileOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let as_user = parse::optional_username(params.0.user)?;
        let path = self.confine_path(&lease_id, &as_user, &params.0.path).await?;
        let data = self.client()?.read_file(&lease_id, path, as_user).await?;
        Ok(Json(ReadFileOutput {
            content_base64: STANDARD.encode(&data),
        }))
    }

    #[tool(
        description = "List a directory in a running lease's sandbox with size, kind, mode and modification time for every entry. Reports `truncated` when max_entries cut the listing."
    )]
    async fn sandbox_list_dir(&self, params: Parameters<ListDirInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::List {
            path: p.path,
            recursive: FileRecursion::from(p.recursive),
            max_entries: p.max_entries,
        };
        self.run_file_op(p.lease_id, p.user, op).await
    }

    #[tool(description = "Stat a path in a running lease's sandbox (kind, size, mode, modification time).")]
    async fn sandbox_stat(&self, params: Parameters<FilePathInput>) -> JsonReply {
        let p = params.0;
        self.run_file_op(p.lease_id, p.user, FileOp::Stat { path: p.path })
            .await
    }

    #[tool(
        description = "Read a byte range of a file in a running lease's sandbox. The reply carries content_base64, the file's total_size, and whether the range was truncated, so large files are read in pieces."
    )]
    async fn sandbox_read_range(&self, params: Parameters<ReadRangeInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::Read {
            path: p.path,
            offset: p.offset,
            length: p.length,
            max_bytes: p.max_bytes,
        };
        self.run_file_op(p.lease_id, p.user, op).await
    }

    #[tool(
        description = "Atomically replace exact text in a file. The file is rewritten only when `find` occurs exactly expected_occurrences times (default 1); otherwise the reply is outcome edit_rejected and nothing is written."
    )]
    async fn sandbox_edit_file(&self, params: Parameters<EditFileInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::Edit {
            path: p.path,
            find: p.find,
            replace: p.replace,
            expected_occurrences: p.expected_occurrences.unwrap_or(1),
        };
        self.run_file_op(p.lease_id, p.user, op).await
    }

    #[tool(
        description = "Search a running lease's sandbox by file name glob and/or content grep, with result limits. Needs a provider that advertises supports_file_search."
    )]
    async fn sandbox_search_files(&self, params: Parameters<SearchFilesInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::Search {
            path: p.path,
            glob: p.glob,
            pattern: p.pattern,
            regex: FileSearchSyntax::from(p.regex),
            case_insensitive: FileCaseSensitivity::from(p.case_insensitive),
            max_results: p.max_results,
            max_file_bytes: p.max_file_bytes,
        };
        self.run_file_op(p.lease_id, p.user, op).await
    }

    #[tool(description = "Remove a file or directory in a running lease's sandbox. Directories need recursive.")]
    async fn sandbox_remove_path(&self, params: Parameters<RemovePathInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::Remove {
            path: p.path,
            recursive: FileRecursion::from(p.recursive),
        };
        self.run_file_op(p.lease_id, p.user, op).await
    }

    #[tool(description = "Create a directory in a running lease's sandbox.")]
    async fn sandbox_mkdir(&self, params: Parameters<MkdirInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::Mkdir {
            path: p.path,
            parents: FileParentCreation::from(p.parents.unwrap_or(true)),
        };
        self.run_file_op(p.lease_id, p.user, op).await
    }

    #[tool(description = "Rename or move a path inside a running lease's sandbox.")]
    async fn sandbox_rename_path(&self, params: Parameters<RenamePathInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::Rename {
            from: p.from,
            to: p.to,
            overwrite: FileOverwrite::from(p.overwrite),
        };
        self.run_file_op(p.lease_id, p.user, op).await
    }

    #[tool(
        description = "Pack a file or directory tree of a running lease's sandbox into one tar stream (archive_base64). Trees over the archive limit fail with too_large."
    )]
    async fn sandbox_archive_get(&self, params: Parameters<ArchiveGetInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::ArchiveGet {
            path: p.path,
            max_bytes: p.max_bytes,
        };
        self.run_file_op(p.lease_id, p.user, op).await
    }

    #[tool(
        description = "Unpack a base64 tar stream into a directory of a running lease's sandbox. Needs a provider that advertises supports_file_archive."
    )]
    async fn sandbox_archive_put(&self, params: Parameters<ArchivePutInput>) -> JsonReply {
        let p = params.0;
        let data = Base64Data::new(p.archive_base64).map_err(|e| ToolError::invalid("archive_base64", e))?;
        self.run_file_op(p.lease_id, p.user, FileOp::ArchivePut { path: p.path, data })
            .await
    }

    #[tool(
        description = "Record the state of a directory tree in a running lease's sandbox as a baseline and return its checkpoint_id for sandbox_changes. Needs a provider that advertises supports_change_tracking."
    )]
    async fn sandbox_checkpoint(&self, params: Parameters<CheckpointInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::Checkpoint {
            path: p.path,
            hash: FileContentHashing::from(p.hash),
        };
        self.run_file_op(p.lease_id, None, op).await
    }

    #[tool(
        description = "List the paths added, modified and deleted in a running lease's sandbox since a checkpoint, or since the lease start when `since` is omitted. The reply is the net difference, so a file created and deleted in between is not listed. Needs a provider that advertises supports_change_tracking."
    )]
    async fn sandbox_changes(&self, params: Parameters<ChangesInput>) -> JsonReply {
        let p = params.0;
        self.run_file_op(
            p.lease_id,
            None,
            FileOp::Changes {
                since: p.since,
                path: p.path,
            },
        )
        .await
    }

    #[tool(
        description = "Start watching a file or directory of a running lease's sandbox and return a watch_id. Poll it with sandbox_watch_poll and release it with sandbox_watch_stop. Needs a provider that advertises supports_file_watch."
    )]
    async fn sandbox_watch_start(&self, params: Parameters<WatchStartInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::WatchStart {
            path: p.path,
            recursive: FileRecursion::from(p.recursive),
        };
        self.run_file_op(p.lease_id, None, op).await
    }

    #[tool(
        description = "Return the create, modify and delete events of a watch with seq greater than after_seq, plus next_seq to pass on the next poll. When no event is pending it waits up to wait_ms (at most 20000) for one, so a loop of polls follows a path without busy waiting. `overflowed` means events were dropped and the caller should re-scan with sandbox_changes."
    )]
    async fn sandbox_watch_poll(&self, params: Parameters<WatchPollInput>) -> JsonReply {
        let p = params.0;
        let op = FileOp::WatchPoll {
            watch_id: p.watch_id,
            after_seq: p.after_seq,
            wait_ms: bounded_watch_wait(p.wait_ms),
        };
        self.run_file_op(p.lease_id, None, op).await
    }

    #[tool(
        description = "Stop a watch started with sandbox_watch_start. Repeating the call is harmless and reports existed false."
    )]
    async fn sandbox_watch_stop(&self, params: Parameters<WatchStopInput>) -> JsonReply {
        let p = params.0;
        self.run_file_op(p.lease_id, None, FileOp::WatchStop { watch_id: p.watch_id })
            .await
    }
}
