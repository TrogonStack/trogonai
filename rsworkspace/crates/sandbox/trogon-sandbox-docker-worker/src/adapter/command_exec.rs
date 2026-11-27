use super::*;

impl DockerWorkerProvider {
    /// Runs `command` inside `lease_id`'s running container via the Docker
    /// exec API, collecting stdout/stderr and the exit code. Bounded by
    /// `timeout_seconds`; exceeding it kills the exec's process tree and
    /// returns a `timed_out` output with exit code -1, the same shape the
    /// detached path reports.
    ///
    /// When `stream_output` is set, each chunk of output is published under
    /// the execution id as it is read, in addition to being buffered into the
    /// returned `CommandOutput` exactly as before. The id is generated here
    /// because this synchronous path has none of its own; it also tags the
    /// exec's environment so a timeout can find and signal the whole tree.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_command_for_lease(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        cwd: Option<String>,
        env: &CommandEnv,
        timeout_seconds: u32,
        run_as_user: Option<&SandboxUserName>,
        stream_output: StreamedOutput,
        max_output_bytes: usize,
    ) -> anyhow::Result<CommandOutput> {
        self.run_command_for_lease_redacting(
            lease_id,
            command,
            cwd,
            env,
            timeout_seconds,
            run_as_user,
            stream_output,
            max_output_bytes,
            &RedactionSet::default(),
        )
        .await
    }

    /// [`Self::run_command_for_lease`] that replaces every value in
    /// `redaction` in the returned output and the streamed chunks. The
    /// timeout reply carries no command output, so it needs none.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_command_for_lease_redacting(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        cwd: Option<String>,
        env: &CommandEnv,
        timeout_seconds: u32,
        run_as_user: Option<&SandboxUserName>,
        stream_output: StreamedOutput,
        max_output_bytes: usize,
        redaction: &RedactionSet,
    ) -> anyhow::Result<CommandOutput> {
        let container_id = self.active_container_id(lease_id).await?;
        let cwd = self.exec_cwd(&container_id, cwd, run_as_user).await?;
        let execution_id = ExecutionId::new(crate::fresh_id::fresh_id());
        let stream_output = stream_output.is_enabled();
        let streaming = stream_output.then(|| StreamingContext::new(self.js.clone(), lease_id, &execution_id));
        self.executions.lock().await.insert(
            execution_id.clone(),
            ExecutionState::Running {
                lease_id: lease_id.clone(),
                exec_id: None,
                container_id: container_id.clone(),
                stdin: None,
                task: None,
                stdin_task: None,
                uncertainty: None,
            },
        );
        let mut env = env.as_map().clone();
        env.insert(EXECUTION_ID_ENV.to_string(), execution_id.as_str().to_owned());
        let docker = self.docker.clone();
        let executions = Arc::clone(&self.executions);
        let owner = lease_id.clone();
        let user = run_as_user.map(|user| user.as_str().to_string());
        let redaction = redaction.clone();
        let js = self.js.clone();
        let id = execution_id.clone();
        let (reply, response) = oneshot::channel();
        let (start, started) = oneshot::channel();
        let handle = tokio::spawn(async move {
            if started.await.is_err() {
                return;
            }
            let mut reply = Some(reply);
            let mut completed_by_cleanup = false;
            let fut = Self::exec_collect_capped(
                &docker,
                &container_id,
                command,
                cwd,
                &env,
                ExecStdin::None,
                user.as_deref(),
                Some(ExecCreatedCallback {
                    executions: executions.clone(),
                    execution_id: id.clone(),
                }),
                streaming,
                OutputRetention::Buffer,
                Some(max_output_bytes),
                None,
                &redaction,
            );
            tokio::pin!(fut);
            let output = match tokio::time::timeout(Duration::from_secs(u64::from(timeout_seconds)), &mut fut).await {
                Ok(result) => result,
                Err(_) => match Self::kill_timed_out_exec(&docker, &container_id, &executions, &id).await {
                    Ok(TimeoutCleanup::Killed) => {
                        completed_by_cleanup = true;
                        Ok(Self::timeout_output(timeout_seconds))
                    }
                    Ok(TimeoutCleanup::Exited(exit_code)) => {
                        match tokio::time::timeout(OUTPUT_SETTLE_GRACE, &mut fut).await {
                            Ok(output) => output,
                            Err(_) => {
                                let message = format!(
                                    "command {id} exited {exit_code}, but its output collection is unconfirmed"
                                );
                                Self::record_uncertainty(&executions, &id, message.clone()).await;
                                if let Some(reply) = reply.take() {
                                    let _ = reply.send(Err(message));
                                }
                                fut.await
                            }
                        }
                    }
                    Err(error) => {
                        let message = redaction.redact_str(&format!(
                            "command {id} exceeded its deadline; termination is unconfirmed: {error}"
                        ));
                        Self::record_uncertainty(&executions, &id, message.clone()).await;
                        if let Some(reply) = reply.take() {
                            let _ = reply.send(Err(message));
                        }
                        fut.await
                    }
                },
            };
            let output = output.map_err(|error| anyhow::anyhow!(redaction.redact_str(&error.to_string())));
            if stream_output
                && let Ok(output) = &output
                && (output.timed_out.is_timed_out() || completed_by_cleanup)
            {
                Self::announce_cut_short(&js, &owner, &id, Some(output.exit_code)).await;
            }
            Self::record_command_result(&executions, &owner, &id, &output).await;
            if let Some(reply) = reply {
                let _ = reply.send(output.map_err(|error| error.to_string()));
            }
        });
        let task = self.register_command_task(&execution_id, handle).await;
        let _ = start.send(());
        let output = response
            .await
            .map_err(|error| anyhow::anyhow!(error))?
            .map_err(anyhow::Error::msg);
        if output.is_ok() {
            task.join().await;
            self.executions.lock().await.remove(&execution_id);
        }
        output
    }

    /// Starts `command` inside `lease_id`'s running container via the Docker
    /// exec API without waiting for it to finish, returning an opaque
    /// `execution_id` immediately. The exec runs to completion (or until
    /// `timeout_seconds` elapses, unless `deadline` is [`CommandDeadline::Lease`]) in a background task; its status and, once
    /// finished, its output are retrieved later via
    /// [`Self::poll_command_for_lease`].
    ///
    /// When `stream_output` is set, output chunks are published under this
    /// execution's own `execution_id` as they are read, in addition to the
    /// existing buffering into the `CommandOutput` a later `poll_command`
    /// retrieves.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_command_for_lease_detached(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        cwd: Option<String>,
        env: &CommandEnv,
        timeout_seconds: u32,
        run_as_user: Option<&SandboxUserName>,
        stream_output: StreamedOutput,
        stdin_mode: StdinMode,
        retention: OutputRetention,
        deadline: CommandDeadline,
        max_output_bytes: usize,
        process: Option<ProcessSpec>,
    ) -> anyhow::Result<ExecutionId> {
        self.run_command_for_lease_detached_redacting(
            lease_id,
            command,
            cwd,
            env,
            timeout_seconds,
            run_as_user,
            stream_output,
            stdin_mode,
            retention,
            deadline,
            max_output_bytes,
            process,
            RedactionSet::default(),
        )
        .await
    }

    /// [`Self::run_command_for_lease_detached`] that replaces every value in
    /// `redaction` in what [`Self::poll_command_for_lease`] later returns and
    /// in the streamed chunks. What is stored for polling is already redacted.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_command_for_lease_detached_redacting(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        cwd: Option<String>,
        env: &CommandEnv,
        timeout_seconds: u32,
        run_as_user: Option<&SandboxUserName>,
        stream_output: StreamedOutput,
        stdin_mode: StdinMode,
        retention: OutputRetention,
        deadline: CommandDeadline,
        max_output_bytes: usize,
        process: Option<ProcessSpec>,
        redaction: RedactionSet,
    ) -> anyhow::Result<ExecutionId> {
        Ok(self
            .run_command_for_lease_detached_redacting_with_origin(
                lease_id,
                command,
                cwd,
                env,
                timeout_seconds,
                run_as_user,
                stream_output,
                stdin_mode,
                retention,
                deadline,
                max_output_bytes,
                process,
                redaction,
            )
            .await?
            .execution_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_command_for_lease_detached_redacting_with_origin(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        cwd: Option<String>,
        env: &CommandEnv,
        timeout_seconds: u32,
        run_as_user: Option<&SandboxUserName>,
        stream_output: StreamedOutput,
        stdin_mode: StdinMode,
        retention: OutputRetention,
        deadline: CommandDeadline,
        max_output_bytes: usize,
        process: Option<ProcessSpec>,
        redaction: RedactionSet,
    ) -> anyhow::Result<trogon_sandbox_contract::RunCommandStartedResponse> {
        let gate = self.authority_gate(lease_id);
        let _guard = gate.lock().await;
        self.start_detached(
            lease_id,
            command,
            cwd,
            env.as_map(),
            timeout_seconds,
            run_as_user,
            stream_output.is_enabled(),
            stdin_mode,
            retention,
            deadline,
            max_output_bytes,
            process,
            redaction,
            None,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start_detached(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        cwd: Option<String>,
        env: &std::collections::BTreeMap<String, String>,
        timeout_seconds: u32,
        run_as_user: Option<&SandboxUserName>,
        stream_output: bool,
        stdin_mode: StdinMode,
        retention: OutputRetention,
        deadline: CommandDeadline,
        max_output_bytes: usize,
        process: Option<ProcessSpec>,
        redaction: RedactionSet,
        predetermined_id: Option<ExecutionId>,
        supervision: Option<supervised_exec::Launch>,
    ) -> anyhow::Result<trogon_sandbox_contract::RunCommandStartedResponse> {
        let container_id = self.active_container_id(lease_id).await?;
        let execution_id = predetermined_id.unwrap_or_else(|| ExecutionId::new(crate::fresh_id::fresh_id()));
        // Held through the registration below so two starts of one name
        // cannot both pass the existence check.
        let mut processes = self.processes.lock().await;
        let tap = process.as_ref().map(|_| OutputTap::default());
        if let Some(name) = process.as_ref().and_then(|p| p.name.as_ref().map(ProcessName::as_str)) {
            let existing = processes
                .iter()
                .find(|(_, m)| m.lease_id == *lease_id && m.name.as_ref().map(ProcessName::as_str) == Some(name))
                .map(|(id, _)| id.clone());
            if let Some(existing) = existing {
                let running = matches!(
                    self.executions.lock().await.get(&existing),
                    Some(ExecutionState::Running { .. })
                );
                if running {
                    return Ok(trogon_sandbox_contract::RunCommandStartedResponse {
                        execution_id: existing,
                        origin: trogon_sandbox_contract::CommandLaunchOrigin::Reused,
                    });
                }
                processes.remove(&existing);
            }
        }
        if let Some(spec) = &process {
            processes.insert(
                execution_id.clone(),
                ProcessMeta {
                    lease_id: lease_id.clone(),
                    name: spec.name.clone(),
                    readiness: spec.readiness.clone(),
                    container_id: container_id.clone(),
                    tap: tap.clone().unwrap_or_default(),
                },
            );
        }
        let supervised = supervision.is_some();
        let (stdin, stdin_handle) = if let Some(supervision) = supervision {
            (ExecStdin::Supervised(Box::new(supervision)), None)
        } else {
            match stdin_mode {
                StdinMode::Closed => (ExecStdin::None, None),
                StdinMode::Interactive => {
                    let (handle, receiver) = StdinHandle::channel();
                    (ExecStdin::Interactive(receiver), Some(handle))
                }
            }
        };
        self.executions.lock().await.insert(
            execution_id.clone(),
            ExecutionState::Running {
                lease_id: lease_id.clone(),
                exec_id: None,
                container_id: container_id.clone(),
                stdin: stdin_handle,
                task: None,
                stdin_task: None,
                uncertainty: None,
            },
        );
        drop(processes);
        self.launches.lock().await.insert(
            execution_id.clone(),
            StoredLaunch {
                controlled_request: None,
                lease_id: lease_id.clone(),
                command: command.clone(),
                cwd: cwd.clone(),
                env: env.clone(),
                timeout_seconds,
                run_as_user: run_as_user.cloned(),
                stream_output,
                stdin_mode,
                retention,
                deadline,
                max_output_bytes,
                process: process.clone(),
                redaction: redaction.clone(),
            },
        );

        let docker = self.docker.clone();
        let executions = Arc::clone(&self.executions);
        let mut env = if supervised {
            std::collections::BTreeMap::new()
        } else {
            env.clone()
        };
        env.insert(EXECUTION_ID_ENV.to_string(), execution_id.as_str().to_owned());
        let exec_execution_id = execution_id.clone();
        let cwd = self.exec_cwd(&container_id, cwd, run_as_user).await?;
        let user = run_as_user.map(SandboxUserName::as_str).map(str::to_string);
        let owner = lease_id.clone();
        let streaming = if stream_output {
            Some(StreamingContext::new(self.js.clone(), lease_id, &execution_id))
        } else {
            None
        };
        let cut_short_js = stream_output.then(|| self.js.clone());
        let (start, started) = oneshot::channel();

        let handle = tokio::spawn(async move {
            if started.await.is_err() {
                return;
            }
            let mut completed_by_cleanup = false;
            let on_exec_created = ExecCreatedCallback {
                executions: Arc::clone(&executions),
                execution_id: exec_execution_id.clone(),
            };
            let fut = Self::exec_collect_capped(
                &docker,
                &container_id,
                command,
                cwd,
                &env,
                stdin,
                user.as_deref(),
                Some(on_exec_created),
                streaming,
                retention,
                Some(max_output_bytes),
                tap,
                &redaction,
            );
            tokio::pin!(fut);
            let result = match if supervised { CommandDeadline::Lease } else { deadline } {
                CommandDeadline::Timeout => {
                    tokio::time::timeout(Duration::from_secs(timeout_seconds as u64), &mut fut).await
                }
                CommandDeadline::Lease => Ok((&mut fut).await),
            };

            let output = match result {
                Ok(result) => result,
                Err(_) => {
                    match Self::kill_timed_out_exec(&docker, &container_id, &executions, &exec_execution_id).await {
                        Ok(TimeoutCleanup::Killed) => {
                            completed_by_cleanup = true;
                            Ok(Self::timeout_output(timeout_seconds))
                        }
                        Ok(TimeoutCleanup::Exited(exit_code)) => {
                            match tokio::time::timeout(OUTPUT_SETTLE_GRACE, &mut fut).await {
                                Ok(output) => output,
                                Err(_) => {
                                    Self::record_uncertainty(
                                        &executions,
                                        &exec_execution_id,
                                        format!("command exited {exit_code}, but its output collection is unconfirmed"),
                                    )
                                    .await;
                                    fut.await
                                }
                            }
                        }
                        Err(error) => {
                            Self::record_uncertainty(
                                &executions,
                                &exec_execution_id,
                                redaction.redact_str(&format!(
                                    "command exceeded its deadline; termination is unconfirmed: {error}"
                                )),
                            )
                            .await;
                            fut.await
                        }
                    }
                }
            };

            let output = output.map_err(|error| anyhow::anyhow!(redaction.redact_str(&error.to_string())));
            if let (Ok(output), Some(js)) = (&output, &cut_short_js)
                && (output.timed_out.is_timed_out() || completed_by_cleanup)
            {
                Self::announce_cut_short(js, &owner, &exec_execution_id, Some(output.exit_code)).await;
            }
            Self::record_command_result(&executions, &owner, &exec_execution_id, &output).await;
        });
        self.register_command_task(&execution_id, handle).await;
        let _ = start.send(());

        Ok(trogon_sandbox_contract::RunCommandStartedResponse {
            execution_id,
            origin: trogon_sandbox_contract::CommandLaunchOrigin::Created,
        })
    }
}
