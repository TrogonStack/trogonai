use std::collections::BTreeMap;

use base64::Engine;
use trogon_sandbox_contract::{CommandOutput, ReadFileRequest, SandboxFileWrite, WriteFilesRequest};
use trogon_sandbox_protocol::{FileNotFoundError, ReadFileError};

use crate::constants::{
    APPEND_CHUNK, DEFAULT_MODE, INSTALL, MAKE_TEMPORARY, MAX_FILE_BYTES, MAX_FILES, MAX_MODE, NOT_FOUND_EXIT,
    READ_CHUNK, READ_CHUNK_BYTES, REMOVE, SCRIPT_OUTPUT_CAP, SCRIPT_TIMEOUT_SECONDS, WRITE_CHUNK_BYTES,
};
use crate::idempotency::{fresh, idempotency_key};
use crate::{CommandInvocation, FileTransferError, LeaseRecord, SpaceId, SpaceProvider};

impl SpaceProvider {
    /// Runs a short script in the guest.
    async fn guest_shell(
        &self,
        sandbox: &SpaceId,
        script: &str,
        args: Vec<String>,
        cap: usize,
    ) -> Result<CommandOutput, FileTransferError> {
        let mut argv = vec!["/bin/sh".to_owned(), "-c".into(), script.into(), "sandbox-files".into()];
        argv.extend(args);
        let key = idempotency_key("files", &[&fresh()]);
        let env = BTreeMap::new();
        let output = self
            .exec_collect(&CommandInvocation {
                sandbox,
                argv: &argv,
                env: &env,
                cwd: None,
                timeout_seconds: SCRIPT_TIMEOUT_SECONDS,
                cap,
                key: &key,
            })
            .await?;
        if output.timed_out.is_timed_out() {
            return Err(FileTransferError::GuestTimedOut);
        }
        Ok(output)
    }

    /// Runs a script that must exit cleanly.
    async fn guest_step(
        &self,
        sandbox: &SpaceId,
        script: &str,
        args: Vec<String>,
    ) -> Result<CommandOutput, FileTransferError> {
        let output = self.guest_shell(sandbox, script, args, SCRIPT_OUTPUT_CAP).await?;
        if output.exit_code == 0 {
            Ok(output)
        } else {
            Err(FileTransferError::GuestFailed)
        }
    }

    fn file_path(record: &LeaseRecord, path: &str) -> Result<String, FileTransferError> {
        if path.is_empty() || path.contains('\0') {
            return Err(FileTransferError::InvalidPath);
        }
        if path.starts_with('/') {
            return Ok(path.to_owned());
        }
        let base = record.cwd.as_deref().ok_or(FileTransferError::RelativePath)?;
        Ok(format!("{}/{path}", base.trim_end_matches('/')))
    }

    /// Writes files into the guest through temporary files, so a failed write never
    /// leaves a half-written target.
    pub async fn write_files(&self, request: &WriteFilesRequest) -> Result<(), FileTransferError> {
        if request.as_user.is_some() {
            return Err(FileTransferError::AsUser);
        }
        let total: usize = request.files.iter().map(|file| file.content.len()).sum();
        if request.files.len() > MAX_FILES || total > MAX_FILE_BYTES {
            return Err(FileTransferError::TooMuchData);
        }
        let record = self.record(&request.lease_id).await?;
        for file in &request.files {
            self.write_one(&record, file).await?;
        }
        Ok(())
    }

    async fn write_one(&self, record: &LeaseRecord, file: &SandboxFileWrite) -> Result<(), FileTransferError> {
        let path = Self::file_path(record, &file.path)?;
        let mode = file.mode.unwrap_or(DEFAULT_MODE);
        if mode > MAX_MODE {
            return Err(FileTransferError::InvalidMode);
        }
        let sandbox = &record.sandbox_id;
        let made = self.guest_step(sandbox, MAKE_TEMPORARY, vec![path.clone()]).await?;
        let temporary = String::from_utf8_lossy(&made.stdout).trim_end_matches('\n').to_owned();
        if temporary.is_empty() {
            return Err(FileTransferError::NoTemporaryFile);
        }
        let written = async {
            for chunk in file.content.chunks(WRITE_CHUNK_BYTES) {
                let encoded = base64::engine::general_purpose::STANDARD.encode(chunk);
                self.guest_step(sandbox, APPEND_CHUNK, vec![temporary.clone(), encoded])
                    .await?;
            }
            self.guest_step(
                sandbox,
                INSTALL,
                vec![temporary.clone(), path.clone(), format!("{mode:o}")],
            )
            .await
            .map(|_| ())
        }
        .await;
        if written.is_err() {
            let _ = self.guest_step(sandbox, REMOVE, vec![temporary]).await;
        }
        written
    }

    /// Reads a file from the guest in pieces.
    pub async fn read_file(&self, request: &ReadFileRequest) -> Result<Vec<u8>, ReadFileError<FileTransferError>> {
        if request.as_user.is_some() {
            return Err(ReadFileError::Other(FileTransferError::AsUser));
        }
        let record = self
            .record(&request.lease_id)
            .await
            .map_err(|error| ReadFileError::Other(error.into()))?;
        let path = Self::file_path(&record, &request.path).map_err(ReadFileError::Other)?;
        tokio::time::timeout(
            self.config.file_read_timeout,
            self.read_pieces(&record.sandbox_id, path),
        )
        .await
        .unwrap_or(Err(ReadFileError::Other(FileTransferError::ReadTimedOut)))
    }

    async fn read_pieces(&self, sandbox: &SpaceId, path: String) -> Result<Vec<u8>, ReadFileError<FileTransferError>> {
        let mut data: Vec<u8> = Vec::new();
        loop {
            let index = data.len() / READ_CHUNK_BYTES;
            let output = self
                .guest_shell(
                    sandbox,
                    READ_CHUNK,
                    vec![path.clone(), index.to_string()],
                    READ_CHUNK_BYTES * 2,
                )
                .await
                .map_err(ReadFileError::Other)?;
            if output.exit_code == NOT_FOUND_EXIT {
                return Err(ReadFileError::NotFound(FileNotFoundError { path }));
            }
            if output.exit_code != 0 || output.truncated.is_truncated() {
                return Err(ReadFileError::Other(FileTransferError::ReadFailed));
            }
            let chunk = base64::engine::general_purpose::STANDARD
                .decode(String::from_utf8_lossy(&output.stdout).trim())
                .map_err(|_| ReadFileError::Other(FileTransferError::NotBase64))?;
            if data.len() + chunk.len() > MAX_FILE_BYTES {
                return Err(ReadFileError::Other(FileTransferError::TooLarge));
            }
            let full = chunk.len() == READ_CHUNK_BYTES;
            data.extend(chunk);
            if !full {
                return Ok(data);
            }
        }
    }
}
