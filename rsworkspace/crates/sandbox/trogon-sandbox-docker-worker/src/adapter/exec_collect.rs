use super::*;

impl DockerWorkerProvider {
    /// Creates and starts a Docker exec, optionally writing `stdin` before
    /// closing the input side, then collects stdout/stderr and polls
    /// `inspect_exec` briefly until the process has finished to obtain the
    /// exit code.
    ///
    /// Takes `docker` explicitly (rather than as a `&self` method) so it can
    /// be called from a detached [`tokio::spawn`]ed task in
    /// [`Self::run_command_for_lease_detached`] without holding a borrow of
    /// `self` across the `.await` points of that background task.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn exec_collect(
        docker: &Docker,
        container_id: &str,
        command: Vec<String>,
        cwd: Option<String>,
        env: &std::collections::BTreeMap<String, String>,
        stdin: ExecStdin,
        user: Option<&str>,
        on_exec_created: Option<ExecCreatedCallback>,
        streaming: Option<StreamingContext>,
        retention: OutputRetention,
    ) -> anyhow::Result<CommandOutput> {
        Self::exec_collect_capped(
            docker,
            container_id,
            command,
            cwd,
            env,
            stdin,
            user,
            on_exec_created,
            streaming,
            retention,
            None,
            None,
            &RedactionSet::default(),
        )
        .await
    }

    /// [`Self::exec_collect`] with a per-stream retention cap. Internal
    /// callers that need the whole output (file reads, signal scripts) pass
    /// no cap; user commands pass one so a runaway command costs bounded
    /// memory.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn exec_collect_capped(
        docker: &Docker,
        container_id: &str,
        command: Vec<String>,
        cwd: Option<String>,
        env: &std::collections::BTreeMap<String, String>,
        stdin: ExecStdin,
        user: Option<&str>,
        on_exec_created: Option<ExecCreatedCallback>,
        streaming: Option<StreamingContext>,
        retention: OutputRetention,
        max_output_bytes: Option<usize>,
        tap: Option<OutputTap>,
        redaction: &RedactionSet,
    ) -> anyhow::Result<CommandOutput> {
        let stdin = match stdin {
            ExecStdin::Supervised(launch) => {
                return supervised_exec::collect(
                    docker,
                    container_id,
                    command,
                    cwd,
                    env,
                    user,
                    on_exec_created,
                    streaming,
                    retention,
                    max_output_bytes,
                    tap,
                    redaction,
                    *launch,
                )
                .await;
            }
            stdin => stdin,
        };
        let env_vec: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let exec = docker
            .create_exec(
                container_id,
                CreateExecOptions {
                    cmd: Some(command),
                    env: if env_vec.is_empty() { None } else { Some(env_vec) },
                    working_dir: cwd,
                    user: user.map(str::to_string),
                    attach_stdin: Some(!matches!(stdin, ExecStdin::None)),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await
            .context("create_exec failed")?;

        let cap = max_output_bytes.unwrap_or(usize::MAX);
        let mut stdout = CappedOutput::new(cap);
        let mut stderr = CappedOutput::new(cap);
        let mut stderr_tail = Vec::new();
        let mut stdin_task = None;

        let started = docker
            .start_exec(&exec.id, None::<StartExecOptions>)
            .await
            .context("start_exec failed")?;
        if let Some(callback) = &on_exec_created {
            callback.record(exec.id.clone()).await;
        }
        match started {
            StartExecResults::Attached { mut output, mut input } => {
                match stdin {
                    ExecStdin::None => {
                        input.shutdown().await.context("close exec stdin failed")?;
                    }
                    ExecStdin::Once(bytes) => {
                        input.write_all(&bytes).await.context("write exec stdin failed")?;
                        input.shutdown().await.context("close exec stdin failed")?;
                    }
                    ExecStdin::Interactive(ops) => {
                        if let Some(callback) = &on_exec_created {
                            stdin_task = callback.spawn_stdin_task(input, ops).await;
                        }
                    }
                    ExecStdin::Supervised(_) => {
                        anyhow::bail!("supervised input is collected separately")
                    }
                }

                let mut sequencer = ChunkSequencer::new();
                let mut sink = OutputSink {
                    streaming: &streaming,
                    retention,
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                    stderr_tail: &mut stderr_tail,
                };
                // Redaction runs before anything is streamed or retained, so a
                // registered value never leaves the worker in either form.
                let mut stdout_filter = redaction.stream();
                let mut stderr_filter = redaction.stream();
                while let Some(chunk) = output.next().await {
                    match chunk.context("exec output stream failed")? {
                        bollard::container::LogOutput::StdOut { message } => {
                            if let Some(tap) = &tap {
                                tap.push(&message);
                            }
                            let safe = stdout_filter.push(&message);
                            sink.accept(&mut sequencer, trogon_sandbox_contract::OutputStream::Stdout, safe)
                                .await;
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            if let Some(tap) = &tap {
                                tap.push(&message);
                            }
                            let safe = stderr_filter.push(&message);
                            sink.accept(&mut sequencer, trogon_sandbox_contract::OutputStream::Stderr, safe)
                                .await;
                        }
                        _ => {}
                    }
                }
                // Bytes held back in case a value straddled the last chunk.
                sink.accept(
                    &mut sequencer,
                    trogon_sandbox_contract::OutputStream::Stdout,
                    stdout_filter.finish(),
                )
                .await;
                sink.accept(
                    &mut sequencer,
                    trogon_sandbox_contract::OutputStream::Stderr,
                    stderr_filter.finish(),
                )
                .await;
            }
            StartExecResults::Detached => {}
        }

        let exit_code = Self::wait_exec_exit_code(docker, &exec.id).await?;
        if let Some(task) = stdin_task {
            task.dispose().await;
        }
        if let Some(ctx) = &streaming {
            Self::publish_exit_marker(ctx, exit_code).await;
        }

        let truncated = if stdout.truncation().is_truncated() || stderr.truncation().is_truncated() {
            OutputTruncation::Truncated
        } else {
            OutputTruncation::Complete
        };
        let stderr = if retention == OutputRetention::StreamOnly {
            stderr_tail
        } else {
            stderr.into_bytes()
        };
        Ok(CommandOutput {
            exit_code,
            stdout: stdout.into_bytes(),
            stderr,
            truncated,
            ..Default::default()
        })
    }

    /// Publishes one incremental chunk of a streamed command's output as a
    /// [`trogon_sandbox_contract::CommandOutputChunk`] via jetstream, matching
    /// [`crate::nats_lifecycle::NatsLifecycleEmitter`]'s
    /// `self.js.publish(subject, payload.into())` convention for evidence
    /// publishing (as opposed to a plain core-NATS `client.publish`, which
    /// this codebase reserves for direct request/reply responses).
    pub(super) async fn publish_output_chunk(
        ctx: &StreamingContext,
        sequencer: &mut ChunkSequencer,
        stream: trogon_sandbox_contract::OutputStream,
        data: Vec<u8>,
    ) {
        let chunk = trogon_sandbox_contract::CommandOutputChunk {
            execution_id: ctx.execution_id.clone(),
            stream,
            seq: sequencer.next(stream),
            data,
        };
        match serde_json::to_vec(&chunk) {
            Ok(payload) => {
                if let Err(e) = ctx.js.publish(ctx.subject.clone(), payload.into()).await {
                    warn!(subject = %ctx.subject, error = %e, "failed to publish command output chunk");
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to serialize command output chunk");
            }
        }
    }

    /// Announces that a streamed command exited, after its last chunk, so
    /// log followers end at once instead of polling for the exit.
    pub(super) async fn publish_exit_marker(ctx: &StreamingContext, exit_code: i32) {
        let marker = trogon_sandbox_contract::CommandExitMarker {
            execution_id: ctx.execution_id.clone(),
            exit_code: Some(exit_code),
        };
        match serde_json::to_vec(&marker) {
            Ok(payload) => {
                if let Err(e) = ctx.js.publish(ctx.exit_subject.clone(), payload.into()).await {
                    warn!(subject = %ctx.exit_subject, error = %e, "failed to publish command exit marker");
                }
            }
            Err(e) => warn!(error = %e, "failed to serialize command exit marker"),
        }
    }

    /// Announces the exit of a streamed command whose exec was abandoned (timed
    /// out or failed) before it could publish its own marker, so a follower is
    /// released rather than left waiting.
    pub(super) async fn announce_cut_short(
        js: &jetstream::Context,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        exit_code: Option<i32>,
    ) {
        if let Err(e) = trogon_sandbox_protocol::publish_command_exit(js, lease_id, execution_id, exit_code).await {
            warn!(lease_id = %lease_id, execution_id = %execution_id, error = %e, "failed to publish command exit marker");
        }
    }

    /// Polls `inspect_exec` until Docker reports the process has finished
    /// (`running == Some(false)`), since the exit code is not guaranteed to be
    /// populated the instant the output stream closes.
    pub(super) async fn wait_exec_exit_code(docker: &Docker, exec_id: &str) -> anyhow::Result<i32> {
        for _ in 0..100 {
            let inspect = docker.inspect_exec(exec_id).await.context("inspect_exec failed")?;
            if inspect.running == Some(false) {
                return Ok(inspect.exit_code.unwrap_or(-1) as i32);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("timed out waiting for exec {exec_id} to finish")
    }
}
