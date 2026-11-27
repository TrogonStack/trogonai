use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Copy)]
enum Tail {
    CorrelatedAbsent,
    WrongRequest,
    WrongExecution,
    Delivered,
    Output,
    RepeatedAbsence,
    NonAbsentStatus,
    UnsolicitedSignal,
}

struct Api {
    endpoint: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Api {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn request(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    let (end, length) = loop {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await?;
        ensure!(
            count > 0 && bytes.len() + count <= 65536,
            "invalid fixture HTTP request"
        );
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..end])?;
            let length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then_some(value.trim())
                })
                .unwrap_or("0")
                .parse::<usize>()?;
            ensure!(length <= 65536, "fixture HTTP body exceeds its bound");
            break (end + 4, length);
        }
    };
    while bytes.len() < end + length {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await?;
        ensure!(count > 0, "fixture HTTP request ended early");
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(std::str::from_utf8(&bytes[..end])?
        .split_whitespace()
        .nth(1)
        .context("fixture HTTP path is missing")?
        .to_owned())
}

async fn reply(stream: &mut TcpStream, body: serde_json::Value) -> anyhow::Result<()> {
    let body = serde_json::to_vec(&body)?;
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(&body).await?;
    Ok(())
}

async fn events(stream: &mut TcpStream, events: &[EventWire]) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    for event in events {
        serde_json::to_writer(&mut bytes, event)?;
        bytes.push(b'\n');
    }
    let mut header = vec![1, 0, 0, 0];
    header.extend_from_slice(&u32::try_from(bytes.len())?.to_be_bytes());
    stream.write_all(&header).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

async fn control(stream: &mut TcpStream) -> anyhow::Result<CommandWire> {
    let mut bytes = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            return Ok(serde_json::from_slice(&bytes)?);
        }
        ensure!(
            bytes.len() < protocol::MAX_FRAME_BYTES,
            "fixture control exceeds its bound"
        );
        bytes.push(byte);
    }
}

fn absence(id: &ExecutionId) -> EventWire {
    EventWire::FamilyAbsent {
        execution_id: id.clone(),
        released: protocol::ReleaseState::Released,
        root_exit_code: Some(0),
        root_signal: None,
        timed_out: protocol::TimeoutState::WithinDeadline,
        canceled: protocol::Cancellation::Active,
        reaped_count: 1,
        stdout_truncated: protocol::Truncation::Complete,
        stderr_truncated: protocol::Truncation::Complete,
    }
}

async fn supervisor(stream: &mut TcpStream, id: ExecutionId, tail: Tail) -> anyhow::Result<()> {
    stream
        .write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n")
        .await?;
    events(
        stream,
        &[EventWire::Hello {
            version: protocol::VERSION,
            execution_id: id.clone(),
            environment: BTreeMap::new(),
        }],
    )
    .await?;
    ensure!(
        matches!(control(stream).await?, CommandWire::Init { execution_id, .. } if execution_id == id),
        "fixture expected Init"
    );
    events(
        stream,
        &[EventWire::Ready {
            version: protocol::VERSION,
            execution_id: id.clone(),
            helper: Identity {
                pid: 41,
                start_time: 100,
            },
            root: Identity {
                pid: 42,
                start_time: 101,
            },
            process_group: 42,
            session: 42,
        }],
    )
    .await?;
    ensure!(
        matches!(control(stream).await?, CommandWire::Release { execution_id } if execution_id == id),
        "fixture expected Release"
    );
    events(
        stream,
        &[
            EventWire::Released {
                execution_id: id.clone(),
            },
            EventWire::Output {
                execution_id: id.clone(),
                stream: protocol::Stream::Stdout,
                data: b"owned-output".to_vec(),
            },
        ],
    )
    .await?;
    let (request_id, signal) = match control(stream).await? {
        CommandWire::Signal {
            execution_id,
            request_id,
            signal,
        } if execution_id == id => (request_id, signal),
        _ => anyhow::bail!("fixture expected in-flight Signal before absence"),
    };
    events(
        stream,
        &[
            EventWire::RootExited {
                execution_id: id.clone(),
                exit_code: Some(0),
                signal: None,
            },
            absence(&id),
        ],
    )
    .await?;
    let response = EventWire::SignalResult {
        execution_id: if matches!(tail, Tail::WrongExecution) {
            ExecutionId::new("foreign")?
        } else {
            id.clone()
        },
        request_id: if matches!(tail, Tail::WrongRequest) {
            request_id + 1
        } else {
            request_id
        },
        signal,
        delivered: Delivery::from(matches!(tail, Tail::Delivered)),
    };
    let status = EventWire::Status {
        execution_id: id.clone(),
        phase: if matches!(tail, Tail::NonAbsentStatus) {
            protocol::Phase::Running
        } else {
            protocol::Phase::Absent
        },
    };
    let mut trailing = vec![response, status];
    match tail {
        Tail::Output => trailing.push(EventWire::Output {
            execution_id: id.clone(),
            stream: protocol::Stream::Stdout,
            data: b"late".to_vec(),
        }),
        Tail::RepeatedAbsence => trailing.push(absence(&id)),
        Tail::UnsolicitedSignal => trailing.push(EventWire::SignalResult {
            execution_id: id.clone(),
            request_id: request_id + 1,
            signal,
            delivered: Delivery::NotDelivered,
        }),
        _ => {}
    }
    events(stream, &trailing).await?;
    ensure!(
        matches!(control(stream).await?, CommandWire::Acknowledge { execution_id } if execution_id == id),
        "fixture expected absence acknowledgement"
    );
    Ok(())
}

async fn route(stream: &mut TcpStream, id: ExecutionId, tail: Tail) -> anyhow::Result<()> {
    let path = request(stream).await?;
    if path.ends_with("/containers/supervisor-fixture/exec") {
        reply(stream, serde_json::json!({"Id":"supervisor"})).await
    } else if path.ends_with("/exec/supervisor/start") {
        supervisor(stream, id, tail).await
    } else if path.ends_with("/exec/supervisor/json") {
        reply(
            stream,
            serde_json::json!({"ID":"supervisor","Running":false,"ExitCode":0,"Pid":0}),
        )
        .await
    } else {
        anyhow::bail!("unexpected supervisor fixture HTTP route")
    }
}

impl Api {
    async fn start(id: ExecutionId, tail: Tail) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            while let Ok((mut stream, _)) = listener.accept().await {
                let id = id.clone();
                connections.spawn(async move {
                    let _ = route(&mut stream, id, tail).await;
                });
            }
        });
        Ok(Self { endpoint, task })
    }
}

async fn case(tail: Tail) -> anyhow::Result<(anyhow::Result<CommandOutput>, anyhow::Result<bool>, Controller)> {
    let id = ExecutionId::new("controlled-exit-race")?;
    let api = Api::start(id.clone(), tail).await?;
    let docker = Docker::connect_with_http(&api.endpoint, 10, bollard::API_DEFAULT_VERSION)?;
    let request: RunCommandRequest = serde_json::from_value(serde_json::json!({
        "lease_id":"supervisor-fixture", "command":["true"], "detached":true,
    }))?;
    let (controller, launch) = prepare(&trogon_sandbox_contract::ExecutionId::new(id.as_str()), request)?;
    let collector = tokio::spawn(async move {
        collect(
            &docker,
            "supervisor-fixture",
            vec!["supervisor".into()],
            None,
            &BTreeMap::new(),
            None,
            None,
            None,
            OutputRetention::Buffer,
            Some(1024),
            None,
            &RedactionSet::default(),
            launch,
        )
        .await
    });
    controller.ready().await?;
    controller.release().await?;
    let (signal, output) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(controller.signal(15), collector)
    })
    .await
    .context("deterministic supervisor transport fixture exceeded its bound")?;
    Ok((output?, signal, controller))
}

#[tokio::test]
async fn in_flight_signal_after_natural_family_absence_preserves_completed_output() -> anyhow::Result<()> {
    let (output, signal, controller) = case(Tail::CorrelatedAbsent).await?;
    let output = output.context("legitimate in-flight absent signal response invalidated completed output")?;
    ensure!(!signal?, "already absent family reported signal delivery");
    ensure!(
        output.exit_code == 0
            && output.stdout == b"owned-output"
            && output.stderr.is_empty()
            && output.timed_out == CommandTimeoutStatus::WithinDeadline
            && output.truncated == OutputTruncation::Complete,
        "natural completion output changed"
    );
    ensure!(
        !controller.signal(15).await?,
        "completed command did not remain a signal no-op"
    );
    Ok(())
}

#[tokio::test]
async fn malformed_post_absence_frames_cannot_confirm_command_completion() -> anyhow::Result<()> {
    for tail in [
        Tail::WrongRequest,
        Tail::WrongExecution,
        Tail::Delivered,
        Tail::Output,
        Tail::RepeatedAbsence,
        Tail::NonAbsentStatus,
        Tail::UnsolicitedSignal,
    ] {
        let (output, _, _) = case(tail).await?;
        ensure!(
            output.is_err(),
            "malformed post-absence control earned completed output"
        );
    }
    Ok(())
}

#[derive(Default)]
struct WriteState {
    unblocked: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    waker: std::sync::Mutex<Option<std::task::Waker>>,
    bytes: std::sync::Mutex<Vec<u8>>,
    shutdown: std::sync::atomic::AtomicBool,
    dropped: std::sync::atomic::AtomicBool,
}

impl WriteState {
    fn unblock(&self) {
        self.unblocked.store(true, Ordering::Release);
        if let Some(waker) = self
            .waker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            waker.wake();
        }
    }
}

struct BlockedInput(Arc<WriteState>);

impl Drop for BlockedInput {
    fn drop(&mut self) {
        self.0.dropped.store(true, Ordering::Release);
    }
}

impl tokio::io::AsyncWrite for BlockedInput {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if !self.0.unblocked.load(Ordering::Acquire) {
            *self.0.waker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(context.waker().clone());
            self.0.entered.notify_one();
            return std::task::Poll::Pending;
        }
        self.0
            .bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(bytes);
        std::task::Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.0.shutdown.store(true, Ordering::Release);
        std::task::Poll::Ready(Ok(()))
    }
}

fn held_release() -> anyhow::Result<(Controller, Launch)> {
    prepare(
        &trogon_sandbox_contract::ExecutionId::new("held-release"),
        serde_json::from_value(serde_json::json!({
            "lease_id":"supervisor-fixture", "command":["true"], "detached":true,
        }))?,
    )
}

async fn blocked_release(expire: bool) -> anyhow::Result<()> {
    let (controller, launch) = held_release()?;
    let state = Arc::new(WriteState::default());
    let writing = tokio::spawn(writer(
        Box::pin(BlockedInput(state.clone())),
        launch.receiver,
        controller.clone(),
    ));
    let sending = tokio::spawn({
        let controller = controller.clone();
        async move {
            controller
                .send(CommandWire::Release {
                    execution_id: controller.id.clone(),
                })
                .await
        }
    });
    state.entered.notified().await;
    if expire {
        tokio::time::advance(Duration::from_secs(6)).await;
        ensure!(
            sending.await?.is_err(),
            "blocked release did not reach the real control-write deadline"
        );
    }
    controller.close();
    state.unblock();
    tokio::time::timeout(Duration::from_secs(1), writing).await??;
    ensure!(
        state.shutdown.load(Ordering::Acquire) && state.dropped.load(Ordering::Acquire),
        "closed native input was not shut down and dropped"
    );
    ensure!(
        !state
            .bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&b'\n'),
        "closed or expired release completed a late executable frame"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn close_retracts_a_blocked_release_before_its_frame_completes() -> anyhow::Result<()> {
    blocked_release(false).await
}

#[tokio::test(start_paused = true)]
async fn expired_release_cannot_complete_after_controller_closes() -> anyhow::Result<()> {
    blocked_release(true).await
}

#[tokio::test]
async fn a_fully_written_release_remains_uncertain_until_native_acknowledgement() -> anyhow::Result<()> {
    let (controller, launch) = held_release()?;
    let state = Arc::new(WriteState::default());
    state.unblock();
    let writing = tokio::spawn(writer(
        Box::pin(BlockedInput(state.clone())),
        launch.receiver,
        controller.clone(),
    ));
    controller
        .send(CommandWire::Release {
            execution_id: controller.id.clone(),
        })
        .await?;
    ensure!(
        state
            .bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ends_with(b"\n"),
        "fixture never wrote the complete release frame"
    );
    controller.close();
    writing.await?;
    ensure!(
        !controller.released() && controller.completion().is_none(),
        "write completion invented native release or family absence"
    );
    ensure!(
        controller.absent().await.is_err(),
        "fully written but unacknowledged release invented cleanup"
    );
    Ok(())
}
