use std::time::Duration;

use base64::Engine;
use trogon_sandbox_contract::{
    CommandExecution, CommandOutput, ReadFileRequest, RunCommandRequest, StreamedOutput, WriteFilesRequest,
};
use trogon_sandbox_protocol::{
    FileNotFoundError, GUEST_STREAM_LIMIT, ReadFileError, effective_max_output_bytes, guest_capture_script,
    parse_guest_capture, resolve_exec_cwd,
};

use crate::check::ensure;
use crate::constants::{
    APPEND, CAPTURE_MARKER, DEFAULT_FILE_MODE, DISCARD, EXEC_GRACE, FILE_CHUNK, FILES_MARKER, FINISH, GUEST_ENV_BIN,
    GUEST_SHELL_BUDGET, GUEST_TIMEOUT_BIN, MAKE_TEMP, MAX_FILE_BYTES, MAX_FILE_MODE, MAX_FILES, MAX_TIMEOUT_SECONDS,
    READ_TIMEOUT, WRITE_TIMEOUT,
};
use crate::{ProxmoxApiError, ProxmoxProvider, ProxmoxProviderError, VmId};

impl ProxmoxProvider {
    /// Runs one attached command in the lease's VM and captures its output.
    pub async fn run_command(&self, request: &RunCommandRequest) -> Result<CommandOutput, ProxmoxProviderError> {
        let record = self.record(&request.lease_id).await?;
        ensure(
            matches!(request.execution, CommandExecution::Attached)
                && matches!(request.stream_output, StreamedOutput::Disabled)
                && request.run_as_user.is_none(),
            ProxmoxProviderError::Rejected("detached, streamed and per-user commands are unavailable on Proxmox"),
        )?;
        ensure(
            !request.command.is_empty() && (1..=MAX_TIMEOUT_SECONDS).contains(&request.timeout_seconds),
            ProxmoxProviderError::Rejected("a command and a timeout of 1 to 86400 seconds are required"),
        )?;
        let mut env = record.env.clone();
        env.extend(request.env.iter().map(|(name, value)| (name.clone(), value.clone())));
        let mut argv = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            guest_capture_script(GUEST_TIMEOUT_BIN, GUEST_ENV_BIN),
            CAPTURE_MARKER.to_owned(),
            format!("{}s", request.timeout_seconds),
            resolve_exec_cwd(&record.cwd, request.cwd.as_deref()),
        ];
        argv.extend(env.iter().map(|(name, value)| format!("{name}={value}")));
        argv.extend(request.command.iter().cloned());
        let budget = Duration::from_secs(u64::from(request.timeout_seconds)) + EXEC_GRACE;
        let output = self.api.exec(record.vm, &argv, budget).await?;
        ensure(
            output.exitcode == Some(0),
            ProxmoxProviderError::Backend("guest output capture failed"),
        )?;
        let cap = effective_max_output_bytes(request.max_output_bytes, GUEST_STREAM_LIMIT);
        Ok(parse_guest_capture(&output.stdout, request.timeout_seconds, cap)?)
    }

    async fn guest_shell(&self, vm: VmId, script: &str, args: &[&str]) -> Result<String, ProxmoxProviderError> {
        let mut argv: Vec<String> = [
            GUEST_TIMEOUT_BIN,
            "--kill-after=1s",
            "15s",
            "/bin/sh",
            "-c",
            script,
            FILES_MARKER,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        argv.extend(args.iter().map(|arg| (*arg).to_owned()));
        let output = self.api.exec(vm, &argv, GUEST_SHELL_BUDGET).await?;
        ensure(
            output.exitcode == Some(0),
            ProxmoxProviderError::Backend("guest file operation failed"),
        )?;
        Ok(output.stdout)
    }

    /// Writes files atomically: each lands in a temporary sibling and is renamed into place.
    pub async fn write_files(&self, request: &WriteFilesRequest) -> Result<(), ProxmoxProviderError> {
        tokio::time::timeout(WRITE_TIMEOUT, self.write_all(request))
            .await
            .map_err(|_| ProxmoxProviderError::TimedOut("file write"))?
    }

    async fn write_all(&self, request: &WriteFilesRequest) -> Result<(), ProxmoxProviderError> {
        let record = self.record(&request.lease_id).await?;
        ensure(
            request.as_user.is_none(),
            ProxmoxProviderError::Rejected("per-user file writes are unavailable on Proxmox"),
        )?;
        let total: usize = request.files.iter().map(|file| file.content.len()).sum();
        ensure(
            request.files.len() <= MAX_FILES && total <= MAX_FILE_BYTES,
            ProxmoxProviderError::Rejected("too many files or too many bytes"),
        )?;
        for file in &request.files {
            ensure(
                !file.path.is_empty() && !file.path.contains('\0'),
                ProxmoxProviderError::Rejected("file path is empty or contains NUL"),
            )?;
            let mode = file.mode.unwrap_or(DEFAULT_FILE_MODE);
            ensure(
                mode <= MAX_FILE_MODE,
                ProxmoxProviderError::Rejected("file mode is out of range"),
            )?;
            let path = resolve_exec_cwd(&record.cwd, Some(&file.path));
            let made = self.guest_shell(record.vm, MAKE_TEMP, &[&path]).await?;
            let temp = made.trim_end_matches('\n');
            let filled = self.fill(record.vm, temp, &path, &file.content, mode).await;
            if filled.is_err() {
                // Best effort: the temporary file is only litter if this fails too.
                let _ = self.guest_shell(record.vm, DISCARD, &[temp]).await;
            }
            filled?;
        }
        Ok(())
    }

    async fn fill(
        &self,
        vm: VmId,
        temp: &str,
        path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), ProxmoxProviderError> {
        for chunk in content.chunks(FILE_CHUNK) {
            let encoded = base64::engine::general_purpose::STANDARD.encode(chunk);
            self.guest_shell(vm, APPEND, &[temp, &encoded]).await?;
        }
        self.guest_shell(vm, FINISH, &[temp, path, &format!("{mode:o}")])
            .await?;
        Ok(())
    }

    /// Reads a whole file, up to 16 MiB.
    pub async fn read_file(&self, request: &ReadFileRequest) -> Result<Vec<u8>, ReadFileError<ProxmoxProviderError>> {
        self.read_all(request).await.map_err(|error| match error {
            ProxmoxProviderError::Api(ProxmoxApiError::FileNotFound(FileNotFoundError { path })) => {
                ReadFileError::NotFound(FileNotFoundError { path })
            }
            other => ReadFileError::Other(other),
        })
    }

    async fn read_all(&self, request: &ReadFileRequest) -> Result<Vec<u8>, ProxmoxProviderError> {
        let record = self.record(&request.lease_id).await?;
        ensure(
            request.as_user.is_none(),
            ProxmoxProviderError::Rejected("per-user file reads are unavailable on Proxmox"),
        )?;
        let path = resolve_exec_cwd(&record.cwd, Some(&request.path));
        tokio::time::timeout(READ_TIMEOUT, async {
            let mut data = Vec::new();
            loop {
                let (encoded, truncated) = self.api.read_file_chunk(record.vm, &path, data.len()).await?;
                let chunk = base64::engine::general_purpose::STANDARD.decode(encoded)?;
                ensure(
                    data.len() + chunk.len() <= MAX_FILE_BYTES,
                    ProxmoxProviderError::Rejected("file exceeds the read limit"),
                )?;
                ensure(
                    !truncated || !chunk.is_empty(),
                    ProxmoxProviderError::Backend("QEMU file-read made no progress"),
                )?;
                data.extend(chunk);
                if !truncated {
                    return Ok(data);
                }
            }
        })
        .await
        .map_err(|_| ProxmoxProviderError::TimedOut("file read"))?
    }
}
