use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{
    ExecutionId, KillCommandResponse, LeaseError, LeaseId, OutputRetention, RestartCommandResponse, StdinChunk,
    StdinEof, StdinMode, StdinSeq, StopCommandResponse, StopSignal, UnixSignal, WriteStdinResponse,
};

use trogon_sandbox_providers::K8sMicroVmProvider;

use crate::completion::{bound_completion, execution_marker};
use crate::constants::{MARKER_POLL_INTERVAL, MARKER_WAIT, STOP_KILL_SETTLE, STOP_POLL_INTERVAL};
use crate::execution_status::ExecutionStatus;
use crate::execution_tracker::AttachedExec;
use crate::signal_outcome::SignalOutcome;
use crate::stop_outcome::StopOutcome;
use crate::stored_launch::StoredLaunch;
use crate::{CommandBuilder, K8sWorkerError, K8sWorkerProvider, PollOutcome};

/// Kills the process tree of the execution tagged `marker`, so a command that outlived its timeout does not linger.
async fn kill_by_marker(
    inner: Arc<K8sMicroVmProvider>,
    sandbox_id: String,
    marker: String,
) -> Result<(), K8sWorkerError> {
    if inner
        .signal_exec_by_marker(&sandbox_id, &marker, UnixSignal::Kill)
        .await
    {
        Ok(())
    } else {
        Err(K8sWorkerError::TerminationUnconfirmed)
    }
}

impl K8sWorkerProvider {
    /// Starts `command` inside `lease_id`'s running pod without waiting for it
    /// to finish: spawns a background task that runs the (blocking, from the
    /// k8s exec API's perspective) exec to completion and records its result,
    /// returning a generated `execution_id` immediately so the caller can poll
    /// via [`Self::poll_command_execution`]. Returns `None` if `lease_id` has
    /// no active lease on this worker, matching [`Self::exec_in_pod_for_lease`].
    ///
    /// The command is bounded by `timeout` (a command that outlives it is
    /// killed in the pod and polls as `timed_out`) and each retained stream by
    /// `max_output_bytes`. `build` receives the execution id and must put
    /// [`CommandEnv::RESERVED_KEY`](trogon_sandbox_contract::CommandEnv::RESERVED_KEY) in the command's environment,
    /// which is what [`Self::kill_command_for_lease`] and the timeout use to
    /// find the process tree.
    pub async fn start_detached_exec_for_lease(
        &self,
        lease_id: &LeaseId,
        build: CommandBuilder,
        stdin: StdinMode,
        retention: OutputRetention,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Option<String> {
        self.sandbox_for_lease(lease_id).await?;
        let launch = StoredLaunch {
            lease_id: lease_id.clone(),
            build,
            stdin,
            retention,
            timeout,
            max_output_bytes,
        };
        let execution_id = self.spawn_launch(&launch).await?;
        self.launches.lock().await.insert(execution_id.clone(), launch);
        Some(execution_id)
    }

    /// Runs `launch` under a fresh execution id. `None` when its lease is no
    /// longer active on this worker.
    async fn spawn_launch(&self, launch: &StoredLaunch) -> Option<String> {
        let lease_id = launch.lease_id.clone();
        let sandbox_id = self.sandbox_for_lease(&lease_id).await?;
        let inner = Arc::clone(&self.inner);
        let execution_id = uuid::Uuid::now_v7().to_string();
        let command = (launch.build)(&execution_id);
        let marker = execution_marker(&execution_id);
        let StoredLaunch {
            stdin,
            retention,
            timeout,
            max_output_bytes,
            ..
        } = launch.clone();
        let retained = Some((self.executions.clone(), execution_id.clone()));
        if stdin == StdinMode::Closed && retention == OutputRetention::Buffer {
            return Some(
                self.executions
                    .spawn(lease_id, execution_id, move || async move {
                        let executing = inner.clone();
                        let pod = sandbox_id.clone();
                        let completion = Box::pin(async move {
                            Ok(executing.exec_in_pod_capped(&pod, command, max_output_bytes).await?)
                        });
                        bound_completion(
                            completion,
                            timeout,
                            move || kill_by_marker(inner, sandbox_id, marker),
                            retained,
                        )
                        .await
                    })
                    .await,
            );
        }
        Some(
            self.executions
                .spawn_attached(lease_id, execution_id, stdin, move || async move {
                    let exec = inner
                        .exec_in_pod_interactive(&sandbox_id, command, retention, max_output_bytes)
                        .await?;
                    let completion: crate::execution_tracker::Completion =
                        Box::pin(async move { Ok(exec.completion.await?) });
                    let completion = bound_completion(
                        completion,
                        timeout,
                        move || kill_by_marker(inner, sandbox_id, marker),
                        retained,
                    );
                    Ok(AttachedExec {
                        stdin: exec.stdin,
                        completion,
                    })
                })
                .await,
        )
    }

    /// Signals the process tree of a detached command. Returns `None` if
    /// `lease_id` has no active lease on this worker, matching
    /// [`Self::exec_in_pod_for_lease`]. A finished command answers
    /// `AlreadyCompleted`; an unknown one, or one owned by another lease,
    /// `NotFound`.
    pub async fn kill_command_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &str,
        signal: UnixSignal,
    ) -> Option<KillCommandResponse> {
        Some(match self.signal_command(lease_id, execution_id, signal).await? {
            SignalOutcome::Signaled => KillCommandResponse::Signaled,
            SignalOutcome::AlreadyCompleted => KillCommandResponse::AlreadyCompleted,
            SignalOutcome::NotFound => KillCommandResponse::NotFound,
        })
    }

    async fn signal_command(
        &self,
        lease_id: &LeaseId,
        execution_id: &str,
        signal: UnixSignal,
    ) -> Option<SignalOutcome> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        match self.executions.status(lease_id, execution_id).await {
            ExecutionStatus::Unknown => return Some(SignalOutcome::NotFound),
            ExecutionStatus::Finished => return Some(SignalOutcome::AlreadyCompleted),
            ExecutionStatus::Running => {}
        }
        // The process may not have started yet: the marker only shows once
        // the exec is running in the pod, so give it a moment.
        let marker = execution_marker(execution_id);
        let deadline = tokio::time::Instant::now() + MARKER_WAIT;
        loop {
            if self.inner.signal_exec_by_marker(&sandbox_id, &marker, signal).await {
                return Some(SignalOutcome::Signaled);
            }
            match self.executions.status(lease_id, execution_id).await {
                ExecutionStatus::Finished => return Some(SignalOutcome::AlreadyCompleted),
                ExecutionStatus::Unknown => return Some(SignalOutcome::NotFound),
                ExecutionStatus::Running => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Some(SignalOutcome::NotFound);
            }
            tokio::time::sleep(MARKER_POLL_INTERVAL).await;
        }
    }

    /// Waits up to `limit` for `execution_id` to stop running.
    async fn wait_finished(&self, lease_id: &LeaseId, execution_id: &str, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if self.executions.status(lease_id, execution_id).await != ExecutionStatus::Running {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(STOP_POLL_INTERVAL).await;
        }
    }

    /// Ends a detached command gracefully: SIGTERM, then SIGKILL when it is
    /// still running after `grace`. The reply names the signal that ended it.
    /// Returns `None` if `lease_id` has no active lease on this worker. A
    /// finished command answers `AlreadyCompleted`; an unknown one, or one
    /// owned by another lease, `NotFound`.
    pub async fn stop_command_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &str,
        grace: Duration,
    ) -> Option<StopCommandResponse> {
        Some(match self.stop_command(lease_id, execution_id, grace).await? {
            StopOutcome::Stopped(signal) => StopCommandResponse::Stopped { signal },
            StopOutcome::AlreadyCompleted => StopCommandResponse::AlreadyCompleted,
            StopOutcome::NotFound => StopCommandResponse::NotFound,
            StopOutcome::StillRunning => StopCommandResponse::Denied(LeaseError::Backend {
                message: "command still running after SIGKILL".to_string(),
            }),
        })
    }

    async fn stop_command(&self, lease_id: &LeaseId, execution_id: &str, grace: Duration) -> Option<StopOutcome> {
        match self.signal_command(lease_id, execution_id, UnixSignal::Term).await? {
            SignalOutcome::NotFound => return Some(StopOutcome::NotFound),
            SignalOutcome::AlreadyCompleted => return Some(StopOutcome::AlreadyCompleted),
            SignalOutcome::Signaled => {}
        }
        if self.wait_finished(lease_id, execution_id, grace).await {
            return Some(StopOutcome::Stopped(StopSignal::Term));
        }
        match self.signal_command(lease_id, execution_id, UnixSignal::Kill).await? {
            SignalOutcome::Signaled => {}
            // It exited between the two signals, which still counts as ended
            // by the grace-period SIGTERM.
            SignalOutcome::AlreadyCompleted => return Some(StopOutcome::Stopped(StopSignal::Term)),
            SignalOutcome::NotFound => return Some(StopOutcome::NotFound),
        }
        Some(if self.wait_finished(lease_id, execution_id, STOP_KILL_SETTLE).await {
            StopOutcome::Stopped(StopSignal::Kill)
        } else {
            StopOutcome::StillRunning
        })
    }

    /// Runs a detached command again from the request it started with, under
    /// a new execution id. A command still running is stopped first with
    /// `grace`. Returns `None` if `lease_id` has no active lease on this
    /// worker.
    pub async fn restart_command_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &str,
        grace: Duration,
    ) -> Option<RestartCommandResponse> {
        self.sandbox_for_lease(lease_id).await?;
        let launch = match self.launches.lock().await.get(execution_id) {
            Some(l) if &l.lease_id == lease_id => l.clone(),
            _ => return Some(RestartCommandResponse::NotFound),
        };
        match self.stop_command(lease_id, execution_id, grace).await? {
            StopOutcome::Stopped(_) | StopOutcome::AlreadyCompleted => {}
            StopOutcome::NotFound => return Some(RestartCommandResponse::NotFound),
            StopOutcome::StillRunning => {
                return Some(RestartCommandResponse::Denied(LeaseError::Backend {
                    message: "command still running after SIGKILL".to_string(),
                }));
            }
        }
        let new_id = self.spawn_launch(&launch).await?;
        self.launches.lock().await.insert(new_id.clone(), launch);
        Some(RestartCommandResponse::Restarted {
            execution_id: ExecutionId::new(new_id),
        })
    }

    /// Delivers one stdin write to an interactive detached command. Returns
    /// `None` if `lease_id` has no active lease on this worker, matching
    /// [`Self::exec_in_pod_for_lease`].
    pub async fn write_stdin_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &str,
        seq: StdinSeq,
        data: StdinChunk,
        eof: StdinEof,
    ) -> Option<WriteStdinResponse> {
        self.sandbox_for_lease(lease_id).await?;
        Some(
            self.executions
                .write_stdin(lease_id, execution_id, seq, data, eof)
                .await,
        )
    }

    /// Looks up the current state of a detached command previously started
    /// via [`Self::start_detached_exec_for_lease`]. Returns `None` if
    /// `lease_id` has no active lease on this worker (broadcast no-op, same
    /// convention as [`Self::exec_in_pod_for_lease`]); returns
    /// `Some(PollOutcome::NotFound)` if the lease IS owned by this worker but
    /// `execution_id` is unknown or expired. Completed/failed entries are kept
    /// so repeat polls stay idempotent.
    pub async fn poll_command_execution(&self, lease_id: &LeaseId, execution_id: &str) -> Option<PollOutcome> {
        self.sandbox_for_lease(lease_id).await?;
        Some(self.executions.poll(execution_id).await)
    }
}
