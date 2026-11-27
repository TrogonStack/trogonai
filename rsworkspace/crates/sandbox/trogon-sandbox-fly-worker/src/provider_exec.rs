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
    APPEND, DEFAULT_FILE_MODE, DISCARD, EXEC_GRACE, FILE_CHUNK, FINISH, MAKE_TEMP, MAX_FILE_BYTES, MAX_FILE_MODE,
    MAX_FILES, MAX_TIMEOUT_SECONDS, READ_TIMEOUT,
};
use crate::{ExecOutput, FlyApiError, FlyProvider, FlyProviderError, MachineId};

impl FlyProvider {
    /// Runs one attached command in the lease's Machine and captures its output.
    pub async fn run_command(&self, request: &RunCommandRequest) -> Result<CommandOutput, FlyProviderError> {
        let record = self.record(&request.lease_id).await?;
        ensure(
            matches!(request.execution, CommandExecution::Attached)
                && matches!(request.stream_output, StreamedOutput::Disabled)
                && request.run_as_user.is_none(),
            FlyProviderError::Rejected("detached, streamed and per-user commands are unavailable on Fly"),
        )?;
        ensure(
            !request.command.is_empty() && (1..=MAX_TIMEOUT_SECONDS).contains(&request.timeout_seconds),
            FlyProviderError::Rejected("a command and a timeout of 1 to 600 seconds are required"),
        )?;
        let mut env = record.env.clone();
        env.extend(request.env.iter().map(|(name, value)| (name.clone(), value.clone())));
        let mut argv = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            guest_capture_script("timeout", "env"),
            "trogon-sandbox-capture".to_owned(),
            format!("{}s", request.timeout_seconds),
            resolve_exec_cwd(&record.cwd, request.cwd.as_deref()),
        ];
        argv.extend(env.iter().map(|(name, value)| format!("{name}={value}")));
        argv.extend(request.command.iter().cloned());
        let timeout = Duration::from_secs(u64::from(request.timeout_seconds)) + EXEC_GRACE;
        let output = self.api.exec(&record.machine, argv, timeout).await?;
        ensure(output.exit_code == 0, FlyProviderError::Backend("guest capture failed"))?;
        let cap = effective_max_output_bytes(request.max_output_bytes, GUEST_STREAM_LIMIT);
        Ok(parse_guest_capture(&output.stdout, request.timeout_seconds, cap)?)
    }

    async fn guest_shell(
        &self,
        machine: &MachineId,
        script: &str,
        args: &[&str],
    ) -> Result<ExecOutput, FlyProviderError> {
        let mut argv: Vec<String> = [
            "/usr/bin/timeout",
            "--kill-after=1s",
            "15s",
            "/bin/sh",
            "-c",
            script,
            "trogon-sandbox-files",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        argv.extend(args.iter().map(|arg| (*arg).to_owned()));
        let output = self.api.exec(machine, argv, Duration::from_secs(20)).await?;
        ensure(
            output.exit_code == 0,
            FlyProviderError::Backend("guest file operation failed"),
        )?;
        Ok(output)
    }

    /// Writes files atomically: each lands in a temporary sibling and is renamed into place.
    pub async fn write_files(&self, request: &WriteFilesRequest) -> Result<(), FlyProviderError> {
        let record = self.record(&request.lease_id).await?;
        ensure(
            request.as_user.is_none(),
            FlyProviderError::Rejected("per-user file writes are unavailable on Fly"),
        )?;
        let total: usize = request.files.iter().map(|file| file.content.len()).sum();
        ensure(
            request.files.len() <= MAX_FILES && total <= MAX_FILE_BYTES,
            FlyProviderError::Rejected("too many files or too many bytes"),
        )?;
        for file in &request.files {
            ensure(
                !file.path.is_empty() && !file.path.contains('\0'),
                FlyProviderError::Rejected("file path is empty or contains NUL"),
            )?;
            let mode = file.mode.unwrap_or(DEFAULT_FILE_MODE);
            ensure(
                mode <= MAX_FILE_MODE,
                FlyProviderError::Rejected("file mode is out of range"),
            )?;
            let path = resolve_exec_cwd(&record.cwd, Some(&file.path));
            let made = self.guest_shell(&record.machine, MAKE_TEMP, &[&path]).await?;
            let temp = made.stdout.trim();
            let filled = self.fill(&record.machine, temp, &path, &file.content, mode).await;
            if filled.is_err() {
                // Best effort: the temporary file is only litter if this fails too.
                let _ = self.guest_shell(&record.machine, DISCARD, &[temp]).await;
            }
            filled?;
        }
        Ok(())
    }

    async fn fill(
        &self,
        machine: &MachineId,
        temp: &str,
        path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), FlyProviderError> {
        for chunk in content.chunks(FILE_CHUNK) {
            let encoded = base64::engine::general_purpose::STANDARD.encode(chunk);
            self.guest_shell(machine, APPEND, &[temp, &encoded]).await?;
        }
        self.guest_shell(machine, FINISH, &[temp, path, &format!("{mode:o}")])
            .await?;
        Ok(())
    }

    /// Reads a whole file, up to 16 MiB.
    pub async fn read_file(&self, request: &ReadFileRequest) -> Result<Vec<u8>, ReadFileError<FlyProviderError>> {
        self.read_all(request).await.map_err(|error| match error {
            FlyProviderError::Api(FlyApiError::FileNotFound(FileNotFoundError { path })) => {
                ReadFileError::NotFound(FileNotFoundError { path })
            }
            other => ReadFileError::Other(other),
        })
    }

    async fn read_all(&self, request: &ReadFileRequest) -> Result<Vec<u8>, FlyProviderError> {
        let record = self.record(&request.lease_id).await?;
        ensure(
            request.as_user.is_none(),
            FlyProviderError::Rejected("per-user file reads are unavailable on Fly"),
        )?;
        let path = resolve_exec_cwd(&record.cwd, Some(&request.path));
        tokio::time::timeout(READ_TIMEOUT, async {
            let mut data = Vec::new();
            loop {
                let (chunk, full) = self.api.read_file_chunk(&record.machine, &path, data.len()).await?;
                data.extend(chunk);
                ensure(
                    data.len() <= MAX_FILE_BYTES,
                    FlyProviderError::Rejected("file exceeds the read limit"),
                )?;
                if !full {
                    return Ok(data);
                }
            }
        })
        .await
        .map_err(|_| FlyProviderError::TimedOut("file read"))?
    }
}
