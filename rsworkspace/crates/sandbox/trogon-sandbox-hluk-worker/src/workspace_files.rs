//! The file verbs, served from the host side of a lease's workspace through [`FsRoot`], which confines every path
//! to it (symlinks included, since the guest writes there too).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::{
    Base64Data, FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, FileParentCreation, FileResultTruncation,
    FileWriteTruncation, LeaseError, ReadFileRequest, ReadFileResponse, WriteFilesRequest, WriteFilesResponse,
};
use trogon_sandbox_file_ops::{FsRoot, SandboxPrefix};

use crate::constants::{ONE_USER, WORKSPACE_GUEST_DIR};
use crate::{HlukLease, LeaseWorkspace};

fn denied(message: impl Into<String>) -> LeaseError {
    LeaseError::Backend {
        message: message.into(),
    }
}

fn mutates(op: &FileOp) -> bool {
    matches!(
        op,
        FileOp::Write { .. }
            | FileOp::Edit { .. }
            | FileOp::Remove { .. }
            | FileOp::Mkdir { .. }
            | FileOp::Rename { .. }
            | FileOp::ArchivePut { .. }
    )
}

/// Runs `op` on `lease`'s workspace.
async fn execute(lease: &HlukLease, op: FileOp) -> FileOpResponse {
    if lease.kind == LeaseWorkspace::CallerReadOnly && mutates(&op) {
        return FileOpResponse::Failed {
            kind: FileOpErrorKind::PermissionDenied,
            message: "the workspace is a read-only host mount".to_owned(),
        };
    }
    let root = lease.workspace.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let fs = SandboxPrefix::new(WORKSPACE_GUEST_DIR)
            .map_err(|error| error.to_string())
            .and_then(|prefix| FsRoot::new(&root, Some(prefix)).map_err(|error| error.to_string()));
        match fs {
            Ok(fs) => fs.execute(&op),
            Err(message) => FileOpResponse::Failed {
                kind: FileOpErrorKind::Io,
                message: format!("workspace is unavailable: {message}"),
            },
        }
    })
    .await;
    outcome.unwrap_or_else(|error| FileOpResponse::Failed {
        kind: FileOpErrorKind::Io,
        message: format!("file operation task failed: {error}"),
    })
}

fn describe(response: &FileOpResponse) -> String {
    serde_json::to_string(response).unwrap_or_default()
}

/// Serves the `file_op` verb.
pub async fn file_op(lease: &HlukLease, request: FileOpRequest) -> FileOpResponse {
    if request.as_user.is_some() {
        return FileOpResponse::Unsupported {
            message: ONE_USER.to_owned(),
        };
    }
    execute(lease, request.op).await
}

/// Serves the `write_files` verb.
pub async fn write_files(lease: &HlukLease, request: WriteFilesRequest) -> WriteFilesResponse {
    if request.as_user.is_some() {
        return WriteFilesResponse::Denied(denied(ONE_USER));
    }
    for file in request.files {
        let data = match Base64Data::new(STANDARD.encode(&file.content)) {
            Ok(data) => data,
            Err(error) => return WriteFilesResponse::Denied(denied(error.to_string())),
        };
        let op = FileOp::Write {
            path: file.path.clone(),
            data,
            offset: 0,
            truncate: FileWriteTruncation::Truncate,
            mode: file.mode,
            create_parents: FileParentCreation::Create,
        };
        match execute(lease, op).await {
            FileOpResponse::Written { .. } => {}
            other => return WriteFilesResponse::Denied(denied(format!("writing {}: {}", file.path, describe(&other)))),
        }
    }
    WriteFilesResponse::Written
}

/// Serves the `read_file` verb.
pub async fn read_file(lease: &HlukLease, request: ReadFileRequest) -> ReadFileResponse {
    if request.as_user.is_some() {
        return ReadFileResponse::Denied(denied(ONE_USER));
    }
    let op = FileOp::Read {
        path: request.path.clone(),
        offset: 0,
        length: None,
        max_bytes: None,
    };
    match execute(lease, op).await {
        FileOpResponse::Content {
            data,
            truncated: FileResultTruncation::Complete,
            ..
        } => match STANDARD.decode(data.as_str()) {
            Ok(data) => ReadFileResponse::Content { data },
            Err(error) => ReadFileResponse::Denied(denied(error.to_string())),
        },
        FileOpResponse::NotFound { .. } => ReadFileResponse::NotFound,
        other => ReadFileResponse::Denied(denied(format!("reading {}: {}", request.path, describe(&other)))),
    }
}
