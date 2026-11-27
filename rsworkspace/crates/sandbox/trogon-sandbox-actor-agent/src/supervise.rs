use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use trogon_sandbox_contract::{
    CommandExecution, CommandOutputChunk, ExecutionId, OutputStream, RunCommandRequest, RunCommandResponse,
    RunCommandStartedResponse, UnixSignal,
};

use crate::constants::OUTPUT_READ_CHUNK;
use crate::execution::{Execution, ExecutionState};
use crate::output_collector::OutputCollector;
use crate::spawn_failure::spawn_failure;
use crate::stdin_op::StdinOp;
use crate::stdin_writer::run_stdin_writer;
use crate::unix_signals::{deliver_signal, group_of};
use crate::{ExecBegin, ExecFrame};

/// Everything the task that owns one child needs.
pub(crate) struct Supervised {
    pub(crate) execution_id: ExecutionId,
    pub(crate) execution: Arc<Execution>,
    pub(crate) command: Command,
    pub(crate) request: RunCommandRequest,
    pub(crate) frames: mpsc::Sender<ExecFrame>,
    pub(crate) kill: mpsc::Receiver<UnixSignal>,
    pub(crate) stdin_ops: Option<mpsc::Receiver<StdinOp>>,
}

async fn emit(frames: &mpsc::Sender<ExecFrame>, frame: ExecFrame) {
    // A caller that stopped listening does not stop the command; the outcome is still recorded for polling.
    let _ = frames.send(frame).await;
}

/// Runs one command to completion, streaming frames as it goes.
///
/// A detached request emits a single `Started` frame and leaves the command running; an attached request emits any
/// streamed chunks followed by exactly one `Completed` frame.
pub(crate) async fn supervise(job: Supervised) {
    let Supervised {
        execution_id,
        execution,
        mut command,
        request,
        frames,
        mut kill,
        stdin_ops,
    } = job;
    let detached = matches!(request.execution, CommandExecution::Detached { .. });
    let streams = request.stream_output.is_enabled();

    if streams && !detached {
        let begin = ExecBegin {
            begin_execution_id: execution_id.clone(),
        };
        emit(&frames, ExecFrame::Begin(begin)).await;
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let output = spawn_failure(&command, &error);
            *execution.state.lock().await = ExecutionState::Completed(output.clone());
            emit(&frames, ExecFrame::Response(RunCommandResponse::Completed(output))).await;
            return;
        }
    };

    let group = group_of(child.id());
    let (finished_tx, finished_rx) = oneshot::channel::<()>();
    if let (Some(ops), Some(input)) = (stdin_ops, child.stdin.take()) {
        tokio::spawn(run_stdin_writer(input, ops, finished_rx));
    }
    let _finished_tx = finished_tx;

    // A detached caller's response ends at `Started`, so its sender is released while the command keeps running.
    let mut frames = Some(frames);
    let started_sink = if detached { frames.take() } else { None };
    if let Some(sink) = started_sink {
        let started = RunCommandStartedResponse {
            execution_id: execution_id.clone(),
            origin: Default::default(),
        };
        emit(&sink, ExecFrame::Response(RunCommandResponse::Started(started))).await;
    }

    let (out_tx, mut out_rx) = mpsc::channel::<(OutputStream, Vec<u8>)>(64);
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, OutputStream::Stdout, out_tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, OutputStream::Stderr, out_tx.clone());
    }
    drop(out_tx);

    let mut collector = OutputCollector::new(request.output_retention, request.max_output_bytes);
    let mut counters = SeqCounters::default();
    let mut exit_code = None;
    let mut readers_done = false;
    let mut timed_out = false;
    let deadline = tokio::time::sleep(Duration::from_secs(u64::from(request.timeout_seconds)));
    tokio::pin!(deadline);

    while !readers_done || exit_code.is_none() {
        tokio::select! {
            received = out_rx.recv(), if !readers_done => match received {
                None => readers_done = true,
                Some((stream, data)) => {
                    collector.push(stream, &data);
                    if streams && let Some(sink) = frames.as_ref() {
                        let chunk = CommandOutputChunk {
                            execution_id: execution_id.clone(),
                            stream,
                            seq: counters.next(stream),
                            data,
                        };
                        emit(sink, ExecFrame::Chunk(chunk)).await;
                    }
                }
            },
            status = child.wait(), if exit_code.is_none() => {
                exit_code = Some(status.ok().and_then(|status| status.code()).unwrap_or(-1));
            }
            Some(signal) = kill.recv() => deliver_signal(group, signal),
            () = &mut deadline, if !timed_out => {
                timed_out = true;
                deliver_signal(group, UnixSignal::Kill);
            }
        }
    }

    let output = collector.finish(exit_code.unwrap_or(-1), timed_out.then_some(request.timeout_seconds));
    *execution.state.lock().await = ExecutionState::Completed(output.clone());
    if let Some(sink) = frames {
        emit(&sink, ExecFrame::Response(RunCommandResponse::Completed(output))).await;
    }
}

#[derive(Default)]
struct SeqCounters {
    stdout: u64,
    stderr: u64,
}

impl SeqCounters {
    fn next(&mut self, stream: OutputStream) -> u64 {
        let counter = match stream {
            OutputStream::Stdout => &mut self.stdout,
            OutputStream::Stderr => &mut self.stderr,
        };
        let value = *counter;
        *counter += 1;
        value
    }
}

fn spawn_reader<R>(pipe: R, stream: OutputStream, tx: mpsc::Sender<(OutputStream, Vec<u8>)>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(forward(pipe, stream, tx));
}

async fn forward<R: AsyncRead + Unpin>(mut pipe: R, stream: OutputStream, tx: mpsc::Sender<(OutputStream, Vec<u8>)>) {
    let mut buffer = vec![0u8; OUTPUT_READ_CHUNK];
    loop {
        let read = pipe.read(&mut buffer).await.unwrap_or(0);
        if read == 0 || tx.send((stream, buffer[..read].to_vec())).await.is_err() {
            return;
        }
    }
}
