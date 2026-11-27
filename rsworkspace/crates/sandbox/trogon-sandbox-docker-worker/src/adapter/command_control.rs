use super::*;

impl DockerWorkerProvider {
    /// Retrieves the status of a command previously started via
    /// [`Self::run_command_for_lease_detached`]. Returns `None` if
    /// `execution_id` is unknown to this worker (never started here, or the
    /// worker process restarted since) or belongs to a different lease.
    pub async fn poll_command_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
    ) -> Option<PollCommandResponse> {
        match self.executions.lock().await.get(execution_id) {
            Some(state) if state.lease_id() != lease_id => None,
            Some(ExecutionState::Running {
                uncertainty: Some(message),
                ..
            }) => Some(PollCommandResponse::Denied(LeaseError::Backend {
                message: message.clone(),
            })),
            Some(ExecutionState::Running { .. }) => Some(PollCommandResponse::Running),
            Some(ExecutionState::Completed { output, .. }) => Some(PollCommandResponse::Completed(output.clone())),
            None => None,
        }
    }

    /// Lists the named or probed processes `lease_id` started, with whether
    /// each still runs. Ordered by execution id (start order).
    pub async fn list_processes_for_lease(&self, lease_id: &LeaseId) -> ListProcessesResponse {
        let metas: Vec<(ExecutionId, ProcessMeta)> = {
            let processes = self.processes.lock().await;
            let mut v: Vec<_> = processes
                .iter()
                .filter(|(_, m)| m.lease_id == *lease_id)
                .map(|(id, m)| (id.clone(), m.clone()))
                .collect();
            v.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
            v
        };
        let executions = self.executions.lock().await;
        let processes = metas
            .into_iter()
            .map(|(execution_id, meta)| {
                let state = match executions.get(&execution_id) {
                    Some(ExecutionState::Completed { output, .. }) => ProcessState::Exited {
                        exit_code: output.exit_code,
                    },
                    _ => ProcessState::Running,
                };
                ProcessInfo {
                    execution_id,
                    name: meta.name,
                    readiness: meta.readiness,
                    state,
                }
            })
            .collect();
        ListProcessesResponse::Processes { processes }
    }

    /// Waits up to `timeout` for the readiness probe of `execution_id`.
    /// A process that exits before its probe passes is
    /// [`WaitReadyResponse::Failed`]; an output probe that matched before the
    /// process exited still counts as ready.
    pub async fn wait_ready_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        timeout: Duration,
    ) -> WaitReadyResponse {
        let meta = match self.processes.lock().await.get(execution_id) {
            Some(m) if m.lease_id == *lease_id => m.clone(),
            _ => return WaitReadyResponse::NotFound,
        };
        let Some(probe) = meta.readiness.clone() else {
            return WaitReadyResponse::NoProbe;
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let exited = match self.executions.lock().await.get(execution_id) {
                Some(ExecutionState::Completed { output, .. }) => Some(output.exit_code),
                Some(ExecutionState::Running { .. }) => None,
                None => return WaitReadyResponse::NotFound,
            };
            let ready = match &probe {
                ReadinessProbe::OutputContains { needle } => meta.tap.contains(needle.as_str().as_bytes()),
                ReadinessProbe::TcpPort { port } if exited.is_none() => {
                    self.tcp_port_listening(&meta.container_id, port.get()).await
                }
                ReadinessProbe::TcpPort { .. } => false,
            };
            if ready {
                return WaitReadyResponse::Ready;
            }
            if let Some(exit_code) = exited {
                return WaitReadyResponse::Failed { exit_code };
            }
            if tokio::time::Instant::now() >= deadline {
                return WaitReadyResponse::NotReady;
            }
            tokio::time::sleep(READY_POLL_INTERVAL).await;
        }
    }

    pub(super) async fn tcp_port_listening(&self, container_id: &str, port: u16) -> bool {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            TCP_LISTEN_SCRIPT.to_string(),
            "sh".to_string(),
            format!("{port:04X}"),
        ];
        matches!(
            Self::exec_collect(
                &self.docker,
                container_id,
                command,
                None,
                &std::collections::BTreeMap::new(),
                ExecStdin::None,
                None,
                None,
                None,
                OutputRetention::Buffer,
            )
            .await,
            Ok(out) if out.exit_code == 0
        )
    }

    /// Sends `signal` to the process behind a detached command execution
    /// previously started via [`Self::run_command_for_lease_detached`].
    ///
    /// Looks up `execution_id`'s tracked state: a completed execution
    /// returns [`KillCommandResponse::AlreadyCompleted`], an unknown one (or
    /// one owned by another lease) returns [`KillCommandResponse::NotFound`],
    /// and a running one is signaled, returning
    /// [`KillCommandResponse::Signaled`] once the kill exec itself has run
    /// (regardless of the signaled process's own exit code). This method
    /// does not write `ExecutionState::Completed` itself; the original
    /// detached task's `Self::wait_exec_exit_code` polling loop is
    /// expected to observe the process exiting and record completion on its
    /// own.
    pub async fn kill_command_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        signal: UnixSignal,
    ) -> KillCommandResponse {
        let controlled = self.controlled_executions.lock().await.get(execution_id).cloned();
        if let Some(controlled) = controlled {
            if controlled.owner.lease_id != *lease_id {
                return KillCommandResponse::NotFound;
            }
            return self.kill_controlled(&controlled, signal).await;
        }
        let state = self.executions.lock().await.get(execution_id).cloned();
        let (exec_id, container_id) = match state {
            None => return KillCommandResponse::NotFound,
            Some(state) if state.lease_id() != lease_id => return KillCommandResponse::NotFound,
            Some(ExecutionState::Completed { .. }) => return KillCommandResponse::AlreadyCompleted,
            Some(ExecutionState::Running {
                exec_id: Some(exec_id),
                container_id,
                ..
            }) => (exec_id, container_id),
            Some(ExecutionState::Running {
                exec_id: None,
                container_id,
                ..
            }) => match Self::wait_for_exec_id(&self.executions, execution_id).await {
                Some(exec_id) => (exec_id, container_id),
                None => return KillCommandResponse::NotFound,
            },
        };

        match Self::signal_exec(&self.docker, &container_id, &exec_id, execution_id, signal).await {
            SignalOutcome::Signaled => {
                if signal == UnixSignal::Kill {
                    match Self::wait_exec_exit_code(&self.docker, &exec_id).await {
                        Ok(_) => {}
                        Err(error) => {
                            let message = format!("signal sent but termination is unconfirmed: {error}");
                            Self::record_uncertainty(&self.executions, execution_id, message.clone()).await;
                            return KillCommandResponse::Denied(LeaseError::Backend { message });
                        }
                    }
                }
                KillCommandResponse::Signaled
            }
            SignalOutcome::Gone(_) => KillCommandResponse::AlreadyCompleted,
            SignalOutcome::Uncertain(message) => {
                Self::record_uncertainty(&self.executions, execution_id, message.clone()).await;
                KillCommandResponse::Denied(LeaseError::Backend { message })
            }
        }
    }

    /// Waits up to `limit` for `execution_id` to be recorded as finished.
    pub(super) async fn wait_completed(&self, execution_id: &ExecutionId, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            match self.executions.lock().await.get(execution_id) {
                Some(ExecutionState::Running { .. }) => {}
                _ => return true,
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(STOP_POLL_INTERVAL).await;
        }
    }

    /// Ends a detached command gracefully: SIGTERM, then SIGKILL when it is
    /// still running after `grace`. The reply names the signal that ended it.
    /// A completed execution is [`StopCommandResponse::AlreadyCompleted`]; an
    /// unknown one, or one owned by another lease, is
    /// [`StopCommandResponse::NotFound`].
    pub async fn stop_command_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        grace: Duration,
    ) -> StopCommandResponse {
        match self
            .kill_command_for_lease(lease_id, execution_id, UnixSignal::Term)
            .await
        {
            KillCommandResponse::NotFound => return StopCommandResponse::NotFound,
            KillCommandResponse::AlreadyCompleted => return StopCommandResponse::AlreadyCompleted,
            KillCommandResponse::Denied(e) => return StopCommandResponse::Denied(e),
            KillCommandResponse::Signaled => {}
        }
        if self.wait_completed(execution_id, grace).await {
            return StopCommandResponse::Stopped {
                signal: StopSignal::Term,
            };
        }
        match self
            .kill_command_for_lease(lease_id, execution_id, UnixSignal::Kill)
            .await
        {
            KillCommandResponse::Signaled => {}
            // It exited between the two signals, which still counts as ended
            // by the grace-period SIGTERM.
            KillCommandResponse::AlreadyCompleted => {
                return StopCommandResponse::Stopped {
                    signal: StopSignal::Term,
                };
            }
            KillCommandResponse::NotFound => return StopCommandResponse::NotFound,
            KillCommandResponse::Denied(e) => return StopCommandResponse::Denied(e),
        }
        if self.wait_completed(execution_id, STOP_KILL_SETTLE).await {
            StopCommandResponse::Stopped {
                signal: StopSignal::Kill,
            }
        } else {
            StopCommandResponse::Denied(LeaseError::Backend {
                message: "command still running after SIGKILL".to_string(),
            })
        }
    }

    /// Runs a detached command again from the request it started with, under
    /// a new execution id. A command still running is stopped first with
    /// `grace`. A named process keeps its name and readiness probe, so
    /// `wait_ready` on the new execution id re-probes it.
    pub async fn restart_command_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        grace: Duration,
    ) -> RestartCommandResponse {
        let launch = match self.launches.lock().await.get(execution_id) {
            Some(l) if l.lease_id == *lease_id => l.clone(),
            _ => return RestartCommandResponse::NotFound,
        };
        match self.stop_command_for_lease(lease_id, execution_id, grace).await {
            StopCommandResponse::Stopped { .. } | StopCommandResponse::AlreadyCompleted => {}
            StopCommandResponse::NotFound => return RestartCommandResponse::NotFound,
            StopCommandResponse::Unsupported => return RestartCommandResponse::Unsupported,
            StopCommandResponse::Denied(e) => return RestartCommandResponse::Denied(e),
        }
        if let Some(request) = launch.controlled_request {
            return match self.restart_controlled(lease_id, execution_id, *request).await {
                Ok(started) => RestartCommandResponse::Restarted {
                    execution_id: started.execution_id,
                },
                Err(error) => RestartCommandResponse::Denied(error),
            };
        }
        let env = match CommandEnv::new(launch.env) {
            Ok(env) => env,
            Err(error) => {
                return RestartCommandResponse::Denied(LeaseError::InvalidSpec {
                    reason: error.to_string(),
                });
            }
        };
        match self
            .run_command_for_lease_detached_redacting(
                lease_id,
                launch.command,
                launch.cwd,
                &env,
                launch.timeout_seconds,
                launch.run_as_user.as_ref(),
                StreamedOutput::from(launch.stream_output),
                launch.stdin_mode,
                launch.retention,
                launch.deadline,
                launch.max_output_bytes,
                launch.process,
                launch.redaction,
            )
            .await
        {
            Ok(execution_id) => RestartCommandResponse::Restarted { execution_id },
            Err(e) => RestartCommandResponse::Denied(LeaseError::Backend {
                message: format!("restart failed: {e}"),
            }),
        }
    }

    /// Kills the process tree of an execution whose deadline passed. Used by
    /// both the synchronous and detached paths so a timeout never leaves the
    /// exec running.
    pub(super) async fn kill_timed_out_exec(
        docker: &Docker,
        container_id: &str,
        executions: &Mutex<HashMap<ExecutionId, ExecutionState>>,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<TimeoutCleanup> {
        let exec_id = Self::wait_for_exec_id(executions, execution_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Docker exec identity is unconfirmed"))?;
        match Self::signal_exec(docker, container_id, &exec_id, execution_id, UnixSignal::Kill).await {
            SignalOutcome::Gone(exit_code) => Ok(TimeoutCleanup::Exited(exit_code)),
            SignalOutcome::Signaled => Self::wait_exec_exit_code(docker, &exec_id)
                .await
                .map(|_| TimeoutCleanup::Killed),
            SignalOutcome::Uncertain(message) => Err(anyhow::anyhow!(message)),
        }
    }

    /// Polls for the exec id of a running execution that has not recorded
    /// one yet. `None` when the execution finished, vanished, or still has no
    /// id after [`EXEC_ID_WAIT`].
    pub(super) async fn wait_for_exec_id(
        executions: &Mutex<HashMap<ExecutionId, ExecutionState>>,
        execution_id: &ExecutionId,
    ) -> Option<String> {
        let deadline = tokio::time::Instant::now() + EXEC_ID_WAIT;
        loop {
            match executions.lock().await.get(execution_id) {
                Some(ExecutionState::Running {
                    exec_id: Some(exec_id), ..
                }) => return Some(exec_id.clone()),
                Some(ExecutionState::Running { exec_id: None, .. }) => {}
                _ => return None,
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(EXEC_ID_POLL_INTERVAL).await;
        }
    }

    /// Signals every process in the container whose environment carries the
    /// execution's marker, which covers the whole process tree because
    /// children inherit it. Falls back to the exec's host-reported pid when
    /// the scan is unavailable or matches nothing, so a missed scan never
    /// hides a live command.
    pub(super) async fn signal_exec(
        docker: &Docker,
        container_id: &str,
        exec_id: &str,
        execution_id: &ExecutionId,
        signal: UnixSignal,
    ) -> SignalOutcome {
        let script = vec![
            "sh".to_string(),
            "-c".to_string(),
            SIGNAL_BY_MARKER_SCRIPT.to_string(),
            "sh".to_string(),
            format!("{EXECUTION_ID_ENV}={execution_id}"),
            signal.as_str().to_string(),
        ];
        let output = Self::exec_collect(
            docker,
            container_id,
            script,
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            Some(SIGNAL_EXEC_USER),
            None,
            None,
            OutputRetention::Buffer,
        )
        .await;
        match output {
            Ok(output) if output.exit_code == 0 => SignalOutcome::Signaled,
            other => {
                warn!(exec_id, outcome = ?other.as_ref().map(|o| o.exit_code), "marker signal found nothing, falling back to exec pid");
                Self::signal_exec_by_pid(docker, container_id, exec_id, signal).await
            }
        }
    }

    /// Legacy path: resolves the exec's PID via `docker.inspect_exec` and runs `kill
    /// -<SIGNAL> <pid>` as root in the same container. Returns `false` when
    /// the process is already gone, `true` once the kill exec itself has run.
    pub(super) async fn signal_exec_by_pid(
        docker: &Docker,
        container_id: &str,
        exec_id: &str,
        signal: UnixSignal,
    ) -> SignalOutcome {
        let inspect = match docker.inspect_exec(exec_id).await {
            Ok(inspect) => inspect,
            Err(e) => {
                warn!(exec_id, error = %e, "inspect_exec failed while signaling exec");
                return SignalOutcome::Uncertain(format!("cannot inspect command for signaling: {e}"));
            }
        };
        if inspect.running == Some(false) {
            return SignalOutcome::Gone(inspect.exit_code.unwrap_or(-1) as i32);
        }
        let Some(pid) = inspect.pid.filter(|pid| *pid > 0) else {
            return SignalOutcome::Uncertain("the running command has no confirmed process id".into());
        };

        let kill_cmd = vec!["kill".to_string(), format!("-{}", signal.as_str()), pid.to_string()];
        match Self::exec_collect(
            docker,
            container_id,
            kill_cmd,
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            Some(SIGNAL_EXEC_USER),
            None,
            None,
            OutputRetention::Buffer,
        )
        .await
        {
            Ok(output) if output.exit_code == 0 => SignalOutcome::Signaled,
            Ok(output) => match docker.inspect_exec(exec_id).await {
                Ok(inspect) if inspect.running == Some(false) => {
                    SignalOutcome::Gone(inspect.exit_code.unwrap_or(-1) as i32)
                }
                _ => SignalOutcome::Uncertain(format!("signal command exited {}", output.exit_code)),
            },
            Err(error) => {
                warn!(exec_id, %error, "kill exec failed while signaling exec");
                SignalOutcome::Uncertain(format!("signal command failed: {error}"))
            }
        }
    }

    /// Delivers one stdin write to an interactive detached command, applying
    /// writes at most once and in `seq` order via [`StdinSequencer`]. The
    /// sequencer only advances once the process accepted the bytes, so a
    /// write that timed out before it started can be retried with the same
    /// `seq`. A write that failed or timed out after it started leaves stdin
    /// closed for good, since its effect on the process is unknown.
    pub async fn write_stdin_for_lease(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        seq: StdinSeq,
        data: StdinChunk,
        eof: StdinEof,
    ) -> WriteStdinResponse {
        let state = self.executions.lock().await.get(execution_id).cloned();
        let handle = match state {
            None => return WriteStdinResponse::NotFound,
            Some(state) if state.lease_id() != lease_id => return WriteStdinResponse::NotFound,
            Some(ExecutionState::Completed { .. }) => return WriteStdinResponse::AlreadyCompleted,
            Some(ExecutionState::Running { stdin: None, .. }) => {
                return WriteStdinResponse::StdinNotInteractive;
            }
            Some(ExecutionState::Running {
                stdin: Some(handle), ..
            }) => handle,
        };

        let mut sequencer = handle.sequencer.lock().await;
        if handle.is_broken() {
            return WriteStdinResponse::StdinClosed;
        }
        match sequencer.admit(seq, data, eof) {
            StdinAdmit::Duplicate => WriteStdinResponse::Duplicate {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::Buffered => WriteStdinResponse::Buffered {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::OutOfWindow => WriteStdinResponse::OutOfWindow {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::Closed => WriteStdinResponse::StdinClosed,
            StdinAdmit::Apply(writes) => {
                let mut applied = 0;
                let mut failure = None;
                for write in writes {
                    match handle.apply(write).await {
                        Ok(()) => applied += 1,
                        Err(message) => {
                            failure = Some(message);
                            break;
                        }
                    }
                }
                sequencer.commit(applied);
                match failure {
                    None => WriteStdinResponse::Written {
                        next_seq: sequencer.next_seq(),
                    },
                    Some(StdinApplyError::Backpressure) => WriteStdinResponse::Backpressure {
                        next_seq: sequencer.next_seq(),
                    },
                    Some(StdinApplyError::Broken) => WriteStdinResponse::StdinClosed,
                }
            }
        }
    }

    /// Drops every execution `lease_id` owns, so a released lease leaves no
    /// state behind and a later lease cannot observe it.
    pub(super) async fn forget_executions(&self, lease_id: &LeaseId) {
        let controls = {
            let mut controls = self.controlled_executions.lock().await;
            let keys: Vec<_> = controls
                .iter()
                .filter(|(_, control)| control.owner.lease_id == *lease_id)
                .map(|(key, _)| key.clone())
                .collect();
            keys.into_iter()
                .filter_map(|key| controls.remove(&key))
                .collect::<Vec<_>>()
        };
        for control in controls {
            control.controller.close();
        }
        self.port_forwards.lock().await.remove(lease_id.as_str());
        let tasks = {
            let mut executions = self.executions.lock().await;
            let tasks: Vec<_> = executions
                .values()
                .flat_map(|state| match state {
                    ExecutionState::Running {
                        lease_id: owner,
                        task,
                        stdin_task,
                        ..
                    } if owner == lease_id => vec![task.clone(), stdin_task.clone()],
                    _ => Vec::new(),
                })
                .flatten()
                .collect();
            executions.retain(|_, state| state.lease_id() != lease_id);
            tasks
        };
        for task in tasks {
            task.dispose().await;
        }
        self.processes.lock().await.retain(|_, meta| meta.lease_id != *lease_id);
        self.launches
            .lock()
            .await
            .retain(|_, launch| launch.lease_id != *lease_id);
    }
}
