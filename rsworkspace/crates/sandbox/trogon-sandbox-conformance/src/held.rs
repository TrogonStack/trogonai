use std::time::{Duration, Instant};

use serde_json::Value;
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, FileOp, FileOpRequest, FileOpResponse, KillCommandRequest, KillCommandResponse,
    LeaseId, LeaseRequest, ListProcessesRequest, ListProcessesResponse, PollCommandRequest, PollCommandResponse,
    ProviderDispatchPayload, ProviderLease, ReadFileRequest, ReadFileResponse, RestartCommandRequest,
    RestartCommandResponse, RunCommandRequest, RunCommandResponse, SandboxFileWrite, SandboxFreezeRequest,
    SandboxFreezeResponse, StopCommandRequest, StopCommandResponse, UnixSignal, WaitReadyRequest, WaitReadyResponse,
    WriteFilesRequest, WriteFilesResponse,
};
use trogon_sandbox_protocol::subjects::{
    provider_file_op, provider_kill_command, provider_list_processes, provider_pause, provider_poll_command,
    provider_read_file, provider_restart_command, provider_resume, provider_run_command, provider_stop_command,
    provider_wait_ready, provider_write_files,
};

use crate::check_failure::ensure;
use crate::constants::EVAL;
use crate::encoding::{decode, encode, text};
use crate::run_fields::{ProcessSpec, RunBody, RunFields};
use crate::{CheckFailureError, ExecModel, Target, ids, outcomes};

/// A granted lease that is cancelled on [`Held::release`].
pub(crate) struct Held<'a> {
    pub(crate) target: &'a Target,
    pub(crate) id: LeaseId,
    dispatch: ProviderDispatchPayload,
}

impl<'a> Held<'a> {
    pub(crate) async fn acquire(target: &'a Target) -> Result<Held<'a>, CheckFailureError> {
        Self::acquire_request(target, target.lease_request()?).await
    }

    pub(crate) async fn acquire_request(
        target: &'a Target,
        request: LeaseRequest,
    ) -> Result<Held<'a>, CheckFailureError> {
        let id = ids::fresh_lease_id();
        let assignment = target.assignment(&id)?;
        let payload = ProviderDispatchPayload {
            lease_id: id.clone(),
            request,
            correlation_id: None,
            resolved_drives: vec![],
            target: assignment.clone(),
        };
        let reply: Value = target
            .call(target.dispatch_subject(assignment.as_ref()), &payload, target.timeout)
            .await?;
        let lease = outcomes::granted(reply, "lease was not granted")?;
        let admitted = target.admit_grant(&id, &lease);
        if admitted.is_err() && assignment.is_some() {
            target.cancel(&id).await;
        }
        admitted?;
        Ok(Held {
            target,
            id,
            dispatch: payload,
        })
    }

    /// Dispatches this lease's id a second time and returns the grant.
    pub(crate) async fn redispatch(&self) -> Result<ProviderLease, CheckFailureError> {
        let reply: Value = self
            .target
            .call(
                self.target.dispatch_subject(self.dispatch.target.as_ref()),
                &self.dispatch,
                self.target.timeout,
            )
            .await?;
        let lease = outcomes::granted(reply, "re-dispatch was not granted")?;
        self.target.admit_grant(&self.id, &lease)?;
        Ok(lease)
    }

    pub(crate) async fn release(self) {
        self.target.cancel(&self.id).await;
    }

    pub(crate) async fn run(&self, argv: &[&str], detached: bool) -> Result<RunCommandResponse, CheckFailureError> {
        self.run_with(argv, RunFields::detached(detached)).await
    }

    /// Runs `argv` in the foreground with extra request fields such as `cwd`, `env` or `timeout_seconds`.
    pub(crate) async fn run_with(
        &self,
        argv: &[&str],
        fields: RunFields<'_>,
    ) -> Result<RunCommandResponse, CheckFailureError> {
        self.run_within(argv, fields, self.target.timeout).await
    }

    pub(crate) async fn run_within(
        &self,
        argv: &[&str],
        fields: RunFields<'_>,
        timeout: Duration,
    ) -> Result<RunCommandResponse, CheckFailureError> {
        let body = RunBody {
            lease_id: &self.id,
            command: argv,
            fields: &fields,
        };
        let body = serde_json::to_value(body).map_err(CheckFailureError::Encode)?;
        self.send_run(body, timeout).await
    }

    pub(crate) async fn start_process(
        &self,
        argv: &[String],
        process: ProcessSpec<'_>,
    ) -> Result<RunCommandResponse, CheckFailureError> {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let fields = RunFields {
            detached: Some(true),
            process: Some(process),
            ..RunFields::default()
        };
        self.run_with(&argv, fields).await
    }

    async fn send_run(&self, body: Value, timeout: Duration) -> Result<RunCommandResponse, CheckFailureError> {
        let request: RunCommandRequest = serde_json::from_value(body).map_err(CheckFailureError::Encode)?;
        self.target
            .call(provider_run_command(self.target.kind, &self.id), &request, timeout)
            .await
    }

    /// Leaves `key` in the lease: a file for a shell worker, a global for a JS runtime.
    pub(crate) async fn plant_state(&self, key: &str) -> Result<(), CheckFailureError> {
        match self.target.exec {
            ExecModel::JsEval => {
                let source = format!("globalThis[{:?}] = \"kept\";", state_global(key));
                let out = outcomes::completed(self.run_with(&[EVAL, &source], RunFields::default()).await?)?;
                ensure(out.exit_code == 0, "planting state to exit 0", &out)
            }
            _ => {
                let reply = self.write(&state_path(key), b"kept").await?;
                ensure(
                    matches!(reply, WriteFilesResponse::Written),
                    "the state to be written",
                    &reply,
                )
            }
        }
    }

    /// What [`Held::plant_state`] left under `key`, or `None` when the lease answers that it has nothing there.
    pub(crate) async fn read_state(&self, key: &str, timeout: Duration) -> Result<Option<Vec<u8>>, CheckFailureError> {
        match self.target.exec {
            ExecModel::JsEval => {
                let source = format!("console.log(globalThis[{:?}]);", state_global(key));
                let reply = self.run_within(&[EVAL, &source], RunFields::default(), timeout).await?;
                outcomes::state_from_eval(reply)
            }
            _ => outcomes::state_from_file(self.read(&state_path(key), timeout).await?),
        }
    }

    /// The trimmed stdout of a foreground `argv` that must exit 0.
    pub(crate) async fn stdout_of(&self, argv: &[&str], fields: RunFields<'_>) -> Result<String, CheckFailureError> {
        let out = outcomes::completed(self.run_with(argv, fields).await?)?;
        let clean = out.exit_code == 0 && !out.timed_out.is_timed_out();
        let expected = format!("{argv:?} to exit 0");
        ensure(clean, expected, &out)?;
        Ok(text(&out.stdout).trim_end().to_string())
    }

    pub(crate) async fn poll(
        &self,
        execution_id: &ExecutionId,
        timeout: Duration,
    ) -> Result<PollCommandResponse, CheckFailureError> {
        let request = PollCommandRequest {
            lease_id: self.id.clone(),
            execution_id: execution_id.clone(),
        };
        self.target
            .call(provider_poll_command(self.target.kind, &self.id), &request, timeout)
            .await
    }

    pub(crate) async fn kill(&self, execution_id: &ExecutionId) -> Result<KillCommandResponse, CheckFailureError> {
        let request = KillCommandRequest {
            lease_id: self.id.clone(),
            execution_id: execution_id.clone(),
            signal: UnixSignal::Term,
        };
        self.target
            .call(
                provider_kill_command(self.target.kind, &self.id),
                &request,
                self.target.timeout,
            )
            .await
    }

    pub(crate) async fn stop(&self, execution_id: &ExecutionId) -> Result<StopCommandResponse, CheckFailureError> {
        let request = StopCommandRequest {
            lease_id: self.id.clone(),
            execution_id: execution_id.clone(),
            grace_seconds: 2,
        };
        self.target
            .call(
                provider_stop_command(self.target.kind, &self.id),
                &request,
                self.target.timeout,
            )
            .await
    }

    pub(crate) async fn restart(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<RestartCommandResponse, CheckFailureError> {
        let request = RestartCommandRequest {
            lease_id: self.id.clone(),
            execution_id: execution_id.clone(),
            grace_seconds: 2,
        };
        self.target
            .call(
                provider_restart_command(self.target.kind, &self.id),
                &request,
                self.target.timeout,
            )
            .await
    }

    pub(crate) async fn write(&self, path: &str, content: &[u8]) -> Result<WriteFilesResponse, CheckFailureError> {
        let request = WriteFilesRequest {
            lease_id: self.id.clone(),
            files: vec![SandboxFileWrite {
                path: path.to_string(),
                content: content.to_vec(),
                mode: None,
            }],
            as_user: None,
        };
        self.target
            .call(
                provider_write_files(self.target.kind, &self.id),
                &request,
                self.target.timeout,
            )
            .await
    }

    pub(crate) async fn read(&self, path: &str, timeout: Duration) -> Result<ReadFileResponse, CheckFailureError> {
        let request = ReadFileRequest {
            lease_id: self.id.clone(),
            path: path.to_string(),
            as_user: None,
        };
        self.target
            .call(provider_read_file(self.target.kind, &self.id), &request, timeout)
            .await
    }

    pub(crate) async fn file_op(&self, op: FileOp) -> Result<FileOpResponse, CheckFailureError> {
        let request = FileOpRequest {
            lease_id: self.id.clone(),
            op,
            as_user: None,
        };
        self.target
            .call(
                provider_file_op(self.target.kind, &self.id),
                &request,
                self.target.timeout,
            )
            .await
    }

    /// A fresh scratch directory path under the target's file root.
    pub(crate) fn scratch(&self) -> String {
        format!(
            "{}/conformance-files-{}",
            self.target.file_root.trim_end_matches('/'),
            ids::fresh()
        )
    }

    pub(crate) async fn write_text(&self, path: &str, body: &str) -> Result<(), CheckFailureError> {
        let reply = self
            .file_op(FileOp::Write {
                path: path.to_string(),
                data: encode(body.as_bytes())?,
                offset: 0,
                truncate: Default::default(),
                mode: None,
                create_parents: Default::default(),
            })
            .await?;
        let length = body.len() as u64;
        ensure(
            matches!(reply, FileOpResponse::Written { bytes, size } if bytes == length && size == length),
            format!("{path} to be written"),
            &reply,
        )
    }

    pub(crate) async fn read_text(&self, path: &str) -> Result<String, CheckFailureError> {
        let reply = self
            .file_op(FileOp::Read {
                path: path.to_string(),
                offset: 0,
                length: None,
                max_bytes: None,
            })
            .await?;
        Ok(text(&decode(&outcomes::content(reply, path)?)?))
    }

    pub(crate) async fn freeze(&self, subject: FreezeVerb) -> Result<SandboxFreezeResponse, CheckFailureError> {
        let request = SandboxFreezeRequest {
            lease_id: self.id.clone(),
        };
        let subject = match subject {
            FreezeVerb::Pause => provider_pause(self.target.kind, &self.id),
            FreezeVerb::Resume => provider_resume(self.target.kind, &self.id),
        };
        self.target.call(subject, &request, self.target.timeout).await
    }

    pub(crate) async fn list_processes(&self) -> Result<ListProcessesResponse, CheckFailureError> {
        let request = ListProcessesRequest {
            lease_id: self.id.clone(),
        };
        self.target
            .call(
                provider_list_processes(self.target.kind, &self.id),
                &request,
                self.target.timeout,
            )
            .await
    }

    pub(crate) async fn wait_ready(
        &self,
        execution_id: &ExecutionId,
        timeout_seconds: u32,
    ) -> Result<WaitReadyResponse, CheckFailureError> {
        let request = WaitReadyRequest {
            lease_id: self.id.clone(),
            execution_id: execution_id.clone(),
            timeout_seconds,
        };
        self.target
            .call(
                provider_wait_ready(self.target.kind, &self.id),
                &request,
                self.target.timeout + Duration::from_secs(u64::from(timeout_seconds)),
            )
            .await
    }

    /// Polls until the command completes.
    pub(crate) async fn wait(&self, execution_id: &ExecutionId) -> Result<CommandOutput, CheckFailureError> {
        let deadline = Instant::now() + self.target.timeout;
        loop {
            let reply = self.poll(execution_id, self.target.timeout).await?;
            if let Some(output) = outcomes::finished(reply)? {
                return Ok(output);
            }
            let in_time = Instant::now() <= deadline;
            ensure(in_time, "the command to complete in time", &execution_id)?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Which way [`Held::freeze`] moves a lease.
#[derive(Clone, Copy)]
pub(crate) enum FreezeVerb {
    Pause,
    Resume,
}

pub(crate) fn state_path(key: &str) -> String {
    format!("/tmp/conformance-{key}.txt")
}

pub(crate) fn state_global(key: &str) -> String {
    format!("conformance_{key}")
}
