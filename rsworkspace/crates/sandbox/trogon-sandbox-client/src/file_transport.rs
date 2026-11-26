use std::time::Duration;

use trogon_sandbox_contract::{
    FileOp, FileOpRequest, FileOpResponse, LeaseError, LeaseId, ReadFileRequest, ReadFileResponse, SandboxFileWrite,
    SandboxUserName, WriteFilesRequest, WriteFilesResponse,
};
use trogon_sandbox_protocol::nats_transfer::MAX_TRANSFER_BYTES;
use trogon_sandbox_protocol::subjects;

use crate::{ClientError, SandboxClient, TransportError};

const FILE_TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);
const FILE_OP_TIMEOUT: Duration = Duration::from_secs(60);

impl SandboxClient {
    /// Writes `files` into `lease_id`'s already-running sandbox filesystem.
    pub async fn write_files(
        &self,
        lease_id: &LeaseId,
        files: Vec<SandboxFileWrite>,
        as_user: Option<SandboxUserName>,
    ) -> Result<(), ClientError> {
        let content_bytes = files.iter().map(|file| file.content.len() as u64).sum();
        let request = WriteFilesRequest {
            lease_id: lease_id.clone(),
            files,
            as_user,
        };
        let payload = serde_json::to_vec(&request).map_err(TransportError::Encode)?;
        if payload.len() > MAX_TRANSFER_BYTES {
            return Err(ClientError::Denied(LeaseError::FilesTooLargeForTransport {
                content_bytes,
                max_payload_bytes: MAX_TRANSFER_BYTES as u64,
            }));
        }
        let subject = subjects::lease_write_files(lease_id);
        let bytes = self.call_bytes(&subject, payload, FILE_TRANSFER_TIMEOUT).await?;
        match crate::sandbox_client::decode(&bytes)? {
            WriteFilesResponse::Written => Ok(()),
            WriteFilesResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Reads a single file at `path` out of `lease_id`'s already-running sandbox filesystem.
    pub async fn read_file(
        &self,
        lease_id: &LeaseId,
        path: String,
        as_user: Option<SandboxUserName>,
    ) -> Result<Vec<u8>, ClientError> {
        let request = ReadFileRequest {
            lease_id: lease_id.clone(),
            path: path.clone(),
            as_user,
        };
        let subject = subjects::lease_read_file(lease_id);
        match self.send(&subject, &request, FILE_TRANSFER_TIMEOUT).await? {
            ReadFileResponse::Content { data } => Ok(data),
            ReadFileResponse::NotFound => Err(ClientError::FileNotFound(path)),
            ReadFileResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Sends one structured file operation and returns the worker's typed reply. Broker denials become
    /// [`ClientError::Denied`], an unsupported provider becomes [`ClientError::FileOpUnsupported`], and a missing
    /// path or a failed operation becomes [`ClientError::FileNotFound`] or [`ClientError::FileOpFailed`]. Anything
    /// else is the success reply for `op`.
    pub async fn file_op(
        &self,
        lease_id: &LeaseId,
        op: FileOp,
        as_user: Option<SandboxUserName>,
    ) -> Result<FileOpResponse, ClientError> {
        let request = FileOpRequest {
            lease_id: lease_id.clone(),
            op,
            as_user,
        };
        let subject = subjects::lease_file_op(lease_id);
        match self.send(&subject, &request, FILE_OP_TIMEOUT).await? {
            FileOpResponse::Denied(error) => Err(ClientError::Denied(error)),
            FileOpResponse::Unsupported { message } => Err(ClientError::FileOpUnsupported(message)),
            FileOpResponse::NotFound { path } => Err(ClientError::FileNotFound(path)),
            FileOpResponse::Failed { kind, message } => Err(ClientError::FileOpFailed { kind, message }),
            other => Ok(other),
        }
    }

    pub(crate) fn unexpected_file_reply(operation: &'static str, reply: FileOpResponse) -> ClientError {
        TransportError::UnexpectedReply {
            operation,
            reply: format!("{reply:?}"),
        }
        .into()
    }
}
