use bytes::Bytes;
use uuid::Uuid;

use crate::shell::quote;
use crate::{DaytonaApi, ExecuteRequest, WorkspaceArchiveError};

impl DaytonaApi {
    /// Archives `directory` inside the sandbox with `tar`, downloads the archive and returns its bytes.
    ///
    /// The archive file inside the sandbox is removed afterwards; a failed removal is logged and does not fail the
    /// call.
    pub async fn archive_directory(&self, id: &str, directory: &str) -> Result<Bytes, WorkspaceArchiveError> {
        let remote_path = format!("/tmp/{}.tar", Uuid::new_v4());
        let command = format!("tar -C {} -cf {} .", quote(directory), quote(&remote_path));
        let reply = self
            .execute(
                id,
                &ExecuteRequest {
                    command,
                    cwd: None,
                    timeout: None,
                },
            )
            .await?;
        let archive = if reply.exit_code == 0 {
            self.download(id, &remote_path)
                .await
                .map_err(WorkspaceArchiveError::from)
        } else {
            Err(WorkspaceArchiveError::TarFailed {
                exit_code: reply.exit_code,
                output: reply.result,
            })
        };
        if let Err(error) = self.delete_file(id, &remote_path).await {
            tracing::warn!(sandbox_id = id, remote_path, %error, "failed to remove the workspace archive from the sandbox");
        }
        archive
    }
}
