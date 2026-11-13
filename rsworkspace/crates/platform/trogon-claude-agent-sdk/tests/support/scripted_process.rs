#![allow(
    dead_code,
    reason = "The shared decorator supports independent runtime contract tests."
)]

//! A decorator over [`TokioProcessSpawner`] that lets a test script the CLI.
//!
//! The decorator records the command the runtime built and then spawns the
//! relay fixture through the native spawner with the socket path followed by
//! the same arguments, and the same environment. The relay connects back over
//! a Unix socket, so the test reads what the runtime writes to the child's
//! stdin and decides what the child writes to stdout and stderr and when it
//! exits. The child returned to the runtime is
//! the native child.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    net::{UnixStream, unix::OwnedWriteHalf},
    process::{Child, Command},
    sync::watch,
};
use trogon_claude_agent_sdk::adapters::{SpawnProcess, TokioProcessSpawner};

const RELAY: &str = env!("CARGO_BIN_EXE_claude-sdk-relay");
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedProcessCommand {
    pub executable: OsString,
    pub arguments: Vec<OsString>,
    pub environment: BTreeMap<OsString, Option<OsString>>,
    pub directory: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptedProcessEvent {
    Spawn,
    StdinClosed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingPipe {
    Stdin,
    Stdout,
    Stderr,
}

struct PumpEnds {
    stdin: DuplexStream,
    stdout: DuplexStream,
    stderr: DuplexStream,
}

#[derive(Default)]
struct Record {
    commands: Vec<CapturedProcessCommand>,
    events: Vec<ScriptedProcessEvent>,
    spawn_failure: Option<io::ErrorKind>,
    missing: Option<MissingPipe>,
    ends: Option<PumpEnds>,
}

struct State {
    record: Mutex<Record>,
    completion: watch::Sender<Option<i32>>,
}

#[derive(Clone)]
pub struct ScriptedProcessObserver {
    state: Arc<State>,
}

impl ScriptedProcessObserver {
    fn lock(&self) -> io::Result<MutexGuard<'_, Record>> {
        self.state
            .record
            .lock()
            .map_err(|_| io::Error::other("scripted process record poisoned"))
    }

    fn record(&self, event: ScriptedProcessEvent) -> io::Result<()> {
        self.lock()?.events.push(event);
        Ok(())
    }

    pub fn events(&self) -> io::Result<Vec<ScriptedProcessEvent>> {
        Ok(self.lock()?.events.clone())
    }

    pub fn commands(&self) -> io::Result<Vec<CapturedProcessCommand>> {
        Ok(self.lock()?.commands.clone())
    }

    /// Makes the child exit with `code` after the output written so far.
    pub fn complete(&self, code: i32) {
        let _ = self.state.completion.send_replace(Some(code));
    }

    /// Waits until `event` has been recorded.
    pub async fn wait_for(&self, event: ScriptedProcessEvent) -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.events()?.contains(&event) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Ok::<_, io::Error>(())
        })
        .await
        .map_err(|_| io::Error::other("scripted process event did not occur"))?
    }
}

pub struct ScriptedProcessHarness {
    /// Reads what the runtime writes to the child's stdin.
    pub stdin: DuplexStream,
    /// Bytes written here reach the child's stdout.
    pub stdout: DuplexStream,
    /// Bytes written here reach the child's stderr.
    pub stderr: DuplexStream,
    pub observer: ScriptedProcessObserver,
}

#[derive(Clone)]
pub struct ScriptedProcessSpawner {
    observer: ScriptedProcessObserver,
}

impl ScriptedProcessSpawner {
    pub fn new() -> (Self, ScriptedProcessHarness) {
        let (completion, _) = watch::channel(None);
        let (stdin, input) = tokio::io::duplex(65536);
        let (output, stdout) = tokio::io::duplex(65536);
        let (error, stderr) = tokio::io::duplex(65536);
        let observer = ScriptedProcessObserver {
            state: Arc::new(State {
                record: Mutex::new(Record {
                    ends: Some(PumpEnds { stdin, stdout, stderr }),
                    ..Record::default()
                }),
                completion,
            }),
        };
        (
            Self {
                observer: observer.clone(),
            },
            ScriptedProcessHarness {
                stdin: input,
                stdout: output,
                stderr: error,
                observer,
            },
        )
    }

    pub fn observer(&self) -> ScriptedProcessObserver {
        self.observer.clone()
    }

    /// Returns `kind` from the next spawn in place of the native call.
    pub fn fail_spawn(&self, kind: io::ErrorKind) -> io::Result<()> {
        self.observer.lock()?.spawn_failure = Some(kind);
        Ok(())
    }

    /// Takes `pipe` out of the spawned child before returning it.
    pub fn omit(&self, pipe: MissingPipe) -> io::Result<()> {
        self.observer.lock()?.missing = Some(pipe);
        Ok(())
    }
}

impl SpawnProcess for ScriptedProcessSpawner {
    type Error = io::Error;

    fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        let captured = capture(command);
        let (missing, ends) = {
            let mut record = self.observer.lock()?;
            record.commands.push(captured.clone());
            record.events.push(ScriptedProcessEvent::Spawn);
            if let Some(kind) = record.spawn_failure.take() {
                return Err(io::Error::from(kind));
            }
            let ends = record
                .ends
                .take()
                .ok_or_else(|| io::Error::other("scripted process already spawned"))?;
            (record.missing, ends)
        };
        let directory = tempfile::tempdir()?;
        let socket = directory.path().join("s");
        let listener = std::os::unix::net::UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let mut relay = Command::new(std::path::absolute(RELAY)?);
        relay.arg(&socket).args(&captured.arguments);
        for (key, value) in &captured.environment {
            match value {
                Some(value) => relay.env(key, value),
                None => relay.env_remove(key),
            };
        }
        // The runtime force-kills the relay on shutdown, possibly while it is writing its coverage
        // profile at exit, which leaves a corrupt profile that fails the merge. The relay runs no
        // crate code, so its profile is discarded.
        relay.env("LLVM_PROFILE_FILE", "/dev/null");
        relay
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = TokioProcessSpawner.spawn(&mut relay)?;
        let connection = accept(&listener)?;
        tokio::spawn(pump(connection, ends, self.observer.clone(), directory));
        match missing {
            Some(MissingPipe::Stdin) => drop(child.stdin.take()),
            Some(MissingPipe::Stdout) => drop(child.stdout.take()),
            Some(MissingPipe::Stderr) => drop(child.stderr.take()),
            None => {}
        }
        Ok(child)
    }
}

fn capture(command: &Command) -> CapturedProcessCommand {
    let command = command.as_std();
    CapturedProcessCommand {
        executable: command.get_program().to_owned(),
        arguments: command.get_args().map(ToOwned::to_owned).collect(),
        environment: command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect(),
        directory: command.get_current_dir().map(ToOwned::to_owned),
    }
}

fn accept(listener: &std::os::unix::net::UnixListener) -> io::Result<UnixStream> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(true)?;
                return UnixStream::from_std(stream);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error),
        }
    }
}

async fn pump(connection: UnixStream, ends: PumpEnds, observer: ScriptedProcessObserver, _directory: TempDir) {
    let (mut from_child, mut to_child) = connection.into_split();
    let PumpEnds {
        stdin: mut input,
        mut stdout,
        mut stderr,
    } = ends;
    let forward = {
        let observer = observer.clone();
        async move {
            let _ = tokio::io::copy(&mut from_child, &mut input).await;
            let _ = input.shutdown().await;
            drop(input);
            let _ = observer.record(ScriptedProcessEvent::StdinClosed);
        }
    };
    let frames = async move {
        let mut completion = observer.state.completion.subscribe();
        let _ = relay_output(&mut to_child, &mut stdout, &mut stderr, &mut completion).await;
    };
    tokio::join!(forward, frames);
}

async fn relay_output(
    to_child: &mut OwnedWriteHalf,
    stdout: &mut DuplexStream,
    stderr: &mut DuplexStream,
    completion: &mut watch::Receiver<Option<i32>>,
) -> io::Result<()> {
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut out = vec![0_u8; 8192];
    let mut err = vec![0_u8; 8192];
    loop {
        tokio::select! {
            biased;
            read = stdout.read(&mut out), if stdout_open => match read {
                Ok(0) | Err(_) => stdout_open = false,
                Ok(count) => send(to_child, b'o', &out[..count]).await?,
            },
            read = stderr.read(&mut err), if stderr_open => match read {
                Ok(0) | Err(_) => stderr_open = false,
                Ok(count) => send(to_child, b'e', &err[..count]).await?,
            },
            code = async { completion.wait_for(Option::is_some).await.map(|code| code.unwrap_or(1)) } => {
                let code = code.map_err(|_| io::Error::other("scripted process completion closed"))?;
                send(to_child, b'x', &code.to_be_bytes()).await?;
                return to_child.shutdown().await;
            }
        }
    }
}

async fn send(to_child: &mut OwnedWriteHalf, tag: u8, payload: &[u8]) -> io::Result<()> {
    let length = u32::try_from(payload.len()).map_err(|_| io::Error::other("relay frame too large"))?;
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(tag);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(payload);
    to_child.write_all(&frame).await
}
