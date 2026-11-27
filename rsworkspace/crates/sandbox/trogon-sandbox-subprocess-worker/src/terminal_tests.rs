use super::*;
use crate::isolation::PassthroughBackend;
use crate::isolation::resource_limits::ResourceLimits;
use crate::launch::EgressPolicy;
use crate::port_forward::{Connector, PortForwardRegistry, Reach};
use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use std::future::Future;
use std::time::Duration;
use trogon_sandbox_contract::PlatformOptions;
use trogon_sandbox_protocol::{terminal_output_bytes, terminal_output_seq};
use trogon_std::env::SystemEnv;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Collect {
    frames: Mutex<Vec<(String, Bytes)>>,
    changed: tokio::sync::Notify,
}

#[async_trait]
impl FrameSink for Collect {
    async fn publish(&self, subject: String, payload: Bytes) {
        locked(&self.frames).push((subject, payload));
        self.changed.notify_waiters();
    }
}

/// The wait for a condition ran out of time.
#[derive(Debug, thiserror::Error)]
#[error("timed out waiting for {0}")]
struct WaitTimedOutError(String);

impl Collect {
    fn output(&self, id: &str) -> Vec<TerminalOutput> {
        let Ok(id) = TerminalId::new(id) else {
            return Vec::new();
        };
        let subject = terminal_output(&id);
        locked(&self.frames)
            .iter()
            .filter(|(s, _)| *s == subject)
            .filter_map(|(_, p)| serde_json::from_slice(p).ok())
            .collect()
    }

    fn text(&self, id: &str) -> String {
        let bytes: Vec<u8> = self
            .output(id)
            .iter()
            .filter_map(terminal_output_bytes)
            .flatten()
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    async fn wait_until(&self, what: &str, mut done: impl FnMut(&Self) -> bool) -> Result<(), WaitTimedOutError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let changed = self.changed.notified();
            if done(self) {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(WaitTimedOutError(what.to_string()));
            }
        }
    }
}

fn lease_spec(workspace: &std::path::Path) -> LaunchSpec {
    LaunchSpec {
        binary: PathBuf::from("/bin/sleep"),
        args: vec!["60".to_string()],
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
        workspace_host_path: workspace.to_path_buf(),
        workspace_sandbox_path: workspace.to_path_buf(),
        workspace_readonly: false,
        egress_policy: EgressPolicy::AllowAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::from_spec(0, 0),
        fs_policy: None,
        platform: PlatformOptions::default(),
        fs_denials: None,
    }
}

struct Fixture {
    terminals: Arc<SubprocessTerminals>,
    sink: Arc<Collect>,
    _workspace: tempfile::TempDir,
}

fn fixture(leases: &[&str]) -> Result<Fixture, Box<dyn std::error::Error>> {
    build_fixture(leases, |sink| SubprocessTerminals::new(sink, &SystemEnv))
}

fn fixture_with_replay(leases: &[&str], replay_bytes: usize) -> Result<Fixture, Box<dyn std::error::Error>> {
    build_fixture(leases, |sink| {
        SubprocessTerminals::with_replay_bytes(sink, replay_bytes)
    })
}

fn build_fixture(
    leases: &[&str],
    terminals: impl FnOnce(Arc<dyn FrameSink>) -> Arc<SubprocessTerminals>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let sink = Arc::new(Collect::default());
    let terminals = terminals(Arc::clone(&sink) as Arc<dyn FrameSink>);
    for lease in leases {
        terminals.register_lease(
            &LeaseId::new(*lease),
            lease_spec(workspace.path()),
            Arc::new(PassthroughBackend::default()),
        );
    }
    Ok(Fixture {
        terminals,
        sink,
        _workspace: workspace,
    })
}

/// A terminal request as it arrives on the wire: the operation's fields sit next to the ids.
fn request(lease: &str, id: &str, op: Value) -> Vec<u8> {
    let mut body = json!({ "lease_id": lease, "terminal_id": id });
    if let (Some(fields), Value::Object(op)) = (body.as_object_mut(), op) {
        fields.extend(op);
    }
    body.to_string().into_bytes()
}

fn open(cmd: &[&str]) -> Value {
    json!({ "op": "open", "cols": 80, "rows": 24, "cmd": cmd })
}

fn input(text: &str) -> Value {
    json!({ "op": "input", "data": STANDARD.encode(text) })
}

fn close() -> Value {
    json!({ "op": "close" })
}

fn opened(id: &str) -> Option<TerminalResponse> {
    TerminalId::new(id)
        .ok()
        .map(|terminal_id| TerminalResponse::Opened { terminal_id })
}

fn exited(sink: &Collect, id: &str) -> bool {
    sink.output(id).iter().any(|f| matches!(f, TerminalOutput::Exit { .. }))
}

#[tokio::test]
async fn a_shell_echoes_input_through_the_pty() -> TestResult {
    let f = fixture(&["l1"])?;
    assert_eq!(f.terminals.handle(&request("l1", "t1", open(&[]))).await, opened("t1"));
    assert_eq!(
        f.terminals
            .handle(&request("l1", "t1", input("echo pty-$((40+2))\r")))
            .await,
        Some(TerminalResponse::Done)
    );
    f.sink
        .wait_until("the echoed output", |s| s.text("t1").contains("pty-42"))
        .await?;
    assert_eq!(
        f.terminals.handle(&request("l1", "t1", close())).await,
        Some(TerminalResponse::Done)
    );
    f.sink.wait_until("the exit frame", |s| exited(s, "t1")).await?;
    assert_eq!(f.terminals.open_count(&LeaseId::new("l1")), 0);
    Ok(())
}

#[tokio::test]
async fn the_command_sees_a_terminal_of_the_requested_size() -> TestResult {
    let f = fixture(&["l1"])?;
    f.terminals.handle(&request("l1", "t1", open(&["/bin/sh"]))).await;
    f.terminals
        .handle(&request("l1", "t1", json!({ "op": "resize", "cols": 99, "rows": 33 })))
        .await;
    f.terminals
        .handle(&request("l1", "t1", input("stty size; tty -s && echo is-a-tty\r")))
        .await;
    f.sink
        .wait_until("the reported size", |s| {
            let text = s.text("t1");
            text.contains("33 99") && text.contains("is-a-tty")
        })
        .await?;
    f.terminals.handle(&request("l1", "t1", close())).await;
    Ok(())
}

#[tokio::test]
async fn a_command_that_exits_ends_with_one_exit_frame_and_its_code() -> TestResult {
    let f = fixture(&["l1"])?;
    f.terminals
        .handle(&request("l1", "t1", open(&["/bin/sh", "-c", "printf done; exit 3"])))
        .await;
    f.sink.wait_until("the exit frame", |s| exited(s, "t1")).await?;
    let frames = f.sink.output("t1");
    let exits: Vec<_> = frames
        .iter()
        .filter(|f| matches!(f, TerminalOutput::Exit { .. }))
        .collect();
    assert_eq!(exits.len(), 1);
    assert!(matches!(exits[0], TerminalOutput::Exit { exit_code: Some(3), .. }));
    assert!(f.sink.text("t1").contains("done"));
    let seqs: Vec<u64> = frames.iter().map(terminal_output_seq).collect();
    assert_eq!(seqs, (0..frames.len() as u64).collect::<Vec<_>>());
    assert_eq!(
        f.terminals.handle(&request("l1", "t1", input("x"))).await,
        Some(TerminalResponse::Failed(TerminalError::NotFound))
    );
    Ok(())
}

#[tokio::test]
async fn open_sets_the_environment_and_directory() -> TestResult {
    let f = fixture(&["l1"])?;
    std::fs::create_dir(f._workspace.path().join("sub dir"))?;
    let op = json!({
        "op": "open",
        "cols": 80,
        "rows": 24,
        "cmd": ["/bin/sh", "-c", "echo \"$GREETING:$TERM:$(basename \"$PWD\")\""],
        "env": { "GREETING": "hello" },
        "cwd": "sub dir",
    });
    assert_eq!(f.terminals.handle(&request("l1", "t1", op)).await, opened("t1"));
    f.sink.wait_until("the exit frame", |s| exited(s, "t1")).await?;
    assert!(
        f.sink.text("t1").contains("hello:xterm-256color:sub dir"),
        "{}",
        f.sink.text("t1")
    );
    Ok(())
}

#[tokio::test]
async fn a_lease_this_worker_does_not_hold_gets_no_reply() -> TestResult {
    let f = fixture(&["l1"])?;
    assert_eq!(f.terminals.handle(&request("l2", "t1", open(&[]))).await, None);
    assert_eq!(f.terminals.handle(b"not json").await, None);
    Ok(())
}

#[tokio::test]
async fn ids_are_not_shared_across_leases_and_reopen_is_idempotent() -> TestResult {
    let f = fixture(&["l1", "l2"])?;
    assert_eq!(f.terminals.handle(&request("l1", "t1", open(&[]))).await, opened("t1"));
    assert_eq!(f.terminals.handle(&request("l1", "t1", open(&[]))).await, opened("t1"));
    assert_eq!(
        f.terminals.handle(&request("l2", "t1", open(&[]))).await,
        Some(TerminalResponse::Failed(TerminalError::IdInUse))
    );
    assert_eq!(
        f.terminals.handle(&request("l2", "t1", input("x"))).await,
        Some(TerminalResponse::Failed(TerminalError::IdInUse))
    );
    assert_eq!(
        f.terminals.handle(&request("l2", "nope", close())).await,
        Some(TerminalResponse::Done)
    );
    f.terminals.handle(&request("l1", "t1", close())).await;
    Ok(())
}

struct NotifyReaped(Arc<tokio::sync::Notify>);

impl Drop for NotifyReaped {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

struct FirstReapBackend {
    first: std::sync::atomic::AtomicBool,
    reaped: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl IsolationBackend for FirstReapBackend {
    async fn launch(&self, spec: &LaunchSpec) -> anyhow::Result<LaunchedProcess> {
        PassthroughBackend::default().launch(spec).await
    }

    async fn launch_terminal(&self, spec: &LaunchSpec, tty: &TerminalStdio) -> anyhow::Result<LaunchedProcess> {
        let mut launched = PassthroughBackend::default().launch_terminal(spec, tty).await?;
        if self.first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            launched.monitors.hold(NotifyReaped(self.reaped.clone()));
        }
        Ok(launched)
    }
}

#[tokio::test]
async fn a_closed_terminals_late_reaper_preserves_the_reopened_id() -> anyhow::Result<()> {
    let workspace = tempfile::tempdir()?;
    let sink = Arc::new(Collect::default());
    let terminals = SubprocessTerminals::new(sink.clone(), &SystemEnv);
    let reaped = Arc::new(tokio::sync::Notify::new());
    terminals.register_lease(
        &LeaseId::new("l1"),
        lease_spec(workspace.path()),
        Arc::new(FirstReapBackend {
            first: std::sync::atomic::AtomicBool::new(true),
            reaped: reaped.clone(),
        }),
    );
    let observed = async {
        anyhow::ensure!(
            terminals
                .handle(&request(
                    "l1",
                    "reused",
                    open(&[
                        "/bin/sh",
                        "-c",
                        "trap '' HUP; printf old-ready; while :; do sleep 1; done",
                    ]),
                ))
                .await
                == opened("reused")
        );
        sink.wait_until("the original process", |s| s.text("reused").contains("old-ready"))
            .await?;
        anyhow::ensure!(terminals.handle(&request("l1", "reused", close())).await == Some(TerminalResponse::Done));
        anyhow::ensure!(
            terminals
                .handle(&request(
                    "l1",
                    "reused",
                    open(&[
                        "/bin/sh",
                        "-c",
                        "printf new-ready; while IFS= read -r line; do printf 'new:%s\\n' \"$line\"; done",
                    ]),
                ))
                .await
                == opened("reused")
        );
        sink.wait_until("the replacement process", |s| s.text("reused").contains("new-ready"))
            .await?;
        tokio::time::timeout(Duration::from_secs(5), reaped.notified()).await?;
        let response = terminals.handle(&request("l1", "reused", input("still-alive\r"))).await;
        anyhow::ensure!(
            response == Some(TerminalResponse::Done),
            "the original terminal's completed reaper removed its live replacement: {response:?}"
        );
        sink.wait_until("input processed by the replacement", |s| {
            s.text("reused").contains("new:still-alive")
        })
        .await?;
        Ok(())
    }
    .await;
    terminals.forget_lease(&LeaseId::new("l1")).await;
    observed
}

struct HeldLaunchBackend {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    reaped: Arc<tokio::sync::Notify>,
}

struct PendingPreview;

#[async_trait]
impl crate::port_forward::Connector for PendingPreview {
    async fn connect(&self, _: u16) -> std::io::Result<Box<dyn crate::port_forward::BridgeIo>> {
        std::future::pending().await
    }
}

struct PauseRevocation {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    timed_out: std::sync::atomic::AtomicBool,
}

impl std::task::Wake for PauseRevocation {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let Some(entered) = locked(&self.entered).take() else {
            return;
        };
        let _ = entered.send(());
        if locked(&self.release).recv_timeout(Duration::from_secs(5)).is_err() {
            self.timed_out.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forgetting_an_old_lease_registration_preserves_its_same_id_replacement() -> anyhow::Result<()> {
    let workspace = tempfile::tempdir()?;
    let sink = Arc::new(Collect::default());
    let terminals = SubprocessTerminals::new(sink.clone(), &SystemEnv);
    let lease = LeaseId::new("reused-lease");
    let unrelated = LeaseId::new("unrelated-lease");
    let forwards = PortForwardRegistry::new();
    let preview = forwards
        .register_with_preview(&lease, Reach::Connector(Arc::new(PendingPreview)))
        .await
        .context("the old lease must have a preview registration")?;
    terminals.register_lease_with_preview(
        &lease,
        lease_spec(workspace.path()),
        Arc::new(PassthroughBackend::default()),
        Some(preview.clone()),
    );
    let (entered, revoked) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::sync_channel(1);
    let pause = Arc::new(PauseRevocation {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(released),
        timed_out: std::sync::atomic::AtomicBool::new(false),
    });
    let waker = std::task::Waker::from(pause.clone());
    let mut connection = Box::pin(preview.connect(18080));
    anyhow::ensure!(
        connection
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
            .is_pending(),
        "the old preview must be waiting when its registration is revoked"
    );
    let forgetting = tokio::spawn({
        let terminals = terminals.clone();
        let lease = lease.clone();
        async move { terminals.forget_lease(&lease).await }
    });
    let replacement_reaped = Arc::new(tokio::sync::Notify::new());
    let unrelated_reaped = Arc::new(tokio::sync::Notify::new());
    let observed = async {
        tokio::time::timeout(Duration::from_secs(5), revoked)
            .await
            .context("old preview revoke did not enter its registered cancellation waker")?
            .context("old preview cancellation waker dropped its handoff")?;
        anyhow::ensure!(!terminals.holds(&lease));
        for (owned, reaped) in [
            (&lease, &replacement_reaped),
            (&unrelated, &unrelated_reaped),
        ] {
            terminals.register_lease(
                owned,
                lease_spec(workspace.path()),
                Arc::new(FirstReapBackend {
                    first: std::sync::atomic::AtomicBool::new(true),
                    reaped: reaped.clone(),
                }),
            );
            anyhow::ensure!(
                terminals
                    .handle(&request(
                        owned.as_str(),
                        owned.as_str(),
                        open(&[
                            "/bin/sh",
                            "-c",
                            "printf replacement-ready; while IFS= read -r line; do printf 'handled:%s\\n' \"$line\"; done",
                        ]),
                    ))
                    .await
                    == opened(owned.as_str())
            );
        }
        sink.wait_until("both live replacement and unrelated terminals", |frames| {
            frames.text(lease.as_str()).contains("replacement-ready")
                && frames.text(unrelated.as_str()).contains("replacement-ready")
        })
        .await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let _ = release.send(());
    let forgotten = tokio::time::timeout(Duration::from_secs(5), forgetting).await;
    drop(connection);
    let replacement = terminals
        .handle(&request(lease.as_str(), lease.as_str(), input("replacement-alive\r")))
        .await;
    let other = terminals
        .handle(&request(
            unrelated.as_str(),
            unrelated.as_str(),
            input("unrelated-alive\r"),
        ))
        .await;
    if replacement == Some(TerminalResponse::Done) && other == Some(TerminalResponse::Done) {
        sink.wait_until("both terminals processing input after old cleanup", |frames| {
            frames.text(lease.as_str()).contains("handled:replacement-alive")
                && frames.text(unrelated.as_str()).contains("handled:unrelated-alive")
        })
        .await?;
    }
    for owned in [&lease, &unrelated] {
        terminals.forget_lease(owned).await;
        forwards.remove(owned).await;
    }
    if observed.is_ok() {
        for (label, reaped) in [("replacement", &replacement_reaped), ("unrelated", &unrelated_reaped)] {
            tokio::time::timeout(Duration::from_secs(5), reaped.notified())
                .await
                .with_context(|| format!("the {label} terminal's exact owned reaper did not finish"))?;
        }
    }
    observed?;
    forgotten
        .context("old forget did not return after its revocation handoff was released")?
        .context("old forget task failed")?;
    anyhow::ensure!(
        !pause.timed_out.load(std::sync::atomic::Ordering::SeqCst),
        "the deterministic revocation handoff exceeded its bound"
    );
    anyhow::ensure!(
        !terminals.holds(&lease)
            && !terminals.holds(&unrelated)
            && terminals.open_count(&lease) == 0
            && terminals.open_count(&unrelated) == 0,
        "all owned registrations and terminals must be retired before the assertion"
    );
    anyhow::ensure!(
        replacement == Some(TerminalResponse::Done) && other == Some(TerminalResponse::Done),
        "old lease cleanup retired a replacement terminal: replacement={replacement:?}, unrelated={other:?}"
    );
    Ok(())
}

#[async_trait]
impl IsolationBackend for HeldLaunchBackend {
    async fn launch(&self, spec: &LaunchSpec) -> anyhow::Result<LaunchedProcess> {
        PassthroughBackend::default().launch(spec).await
    }

    async fn launch_terminal(&self, spec: &LaunchSpec, tty: &TerminalStdio) -> anyhow::Result<LaunchedProcess> {
        let mut launched = PassthroughBackend::default().launch_terminal(spec, tty).await?;
        launched.monitors.hold(NotifyReaped(self.reaped.clone()));
        self.started.notify_one();
        self.release.notified().await;
        Ok(launched)
    }
}

#[tokio::test]
async fn a_terminal_launch_finishing_after_lease_forget_is_reaped_without_admission() -> anyhow::Result<()> {
    let workspace = tempfile::tempdir()?;
    let terminals = SubprocessTerminals::new(Arc::new(Collect::default()), &SystemEnv);
    let backend = Arc::new(HeldLaunchBackend {
        started: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
        reaped: Arc::new(tokio::sync::Notify::new()),
    });
    let lease = LeaseId::new("late-open");
    terminals.register_lease(&lease, lease_spec(workspace.path()), backend.clone());
    let opening = tokio::spawn({
        let terminals = terminals.clone();
        async move { terminals.handle(&request("late-open", "terminal", open(&[]))).await }
    });
    let started = tokio::time::timeout(Duration::from_secs(5), backend.started.notified()).await;
    terminals.forget_lease(&lease).await;
    backend.release.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(5), opening).await??;
    let admitted = terminals.open_count(&lease);
    terminals.forget_lease(&lease).await;
    tokio::time::timeout(Duration::from_secs(5), backend.reaped.notified()).await?;
    started?;
    anyhow::ensure!(
        admitted == 0
            && matches!(
                response,
                Some(TerminalResponse::Failed(TerminalError::SpawnFailed { .. }))
            ),
        "a launch that completed after its exact lease registration was forgotten remained admitted: {response:?}, count={admitted}"
    );
    Ok(())
}

#[tokio::test]
async fn a_lease_holds_at_most_the_terminal_cap() -> TestResult {
    let f = fixture(&["l1"])?;
    for i in 0..MAX_TERMINALS_PER_LEASE {
        let id = format!("t{i}");
        assert_eq!(f.terminals.handle(&request("l1", &id, open(&[]))).await, opened(&id));
    }
    assert_eq!(
        f.terminals.handle(&request("l1", "extra", open(&[]))).await,
        Some(TerminalResponse::Failed(TerminalError::TooManyTerminals {
            max: MAX_TERMINALS_PER_LEASE as u32
        }))
    );
    f.terminals.forget_lease(&LeaseId::new("l1")).await;
    Ok(())
}

#[tokio::test]
async fn forgetting_a_lease_ends_its_terminals() -> TestResult {
    let f = fixture(&["l1"])?;
    f.terminals.handle(&request("l1", "t1", open(&[]))).await;
    f.terminals.forget_lease(&LeaseId::new("l1")).await;
    assert!(exited(&f.sink, "t1"));
    assert_eq!(f.terminals.handle(&request("l1", "t2", open(&[]))).await, None);
    Ok(())
}

#[tokio::test]
async fn a_command_that_cannot_start_fails_to_spawn() -> TestResult {
    let f = fixture(&["l1"])?;
    let response = f
        .terminals
        .handle(&request("l1", "t1", open(&["/nonexistent/binary"])))
        .await;
    assert!(
        matches!(
            response,
            Some(TerminalResponse::Failed(TerminalError::SpawnFailed { .. }))
        ),
        "{response:?}"
    );
    assert_eq!(f.terminals.open_count(&LeaseId::new("l1")), 0);
    Ok(())
}

#[tokio::test]
async fn invalid_environment_names_are_refused() -> TestResult {
    let f = fixture(&["l1"])?;
    let op = json!({
        "op": "open",
        "cols": 80,
        "rows": 24,
        "env": { "A=B": "x" },
    });
    assert!(matches!(
        f.terminals.handle(&request("l1", "t1", op)).await,
        Some(TerminalResponse::Failed(TerminalError::InvalidRequest { .. }))
    ));
    Ok(())
}

#[test]
fn the_spec_runs_the_shell_by_default_and_overrides_lease_env() -> TestResult {
    let mut lease = lease_spec(std::path::Path::new("/w"));
    lease.env.push(("TERM".to_string(), "dumb".to_string()));
    let spec = terminal_spec(
        &lease,
        &[],
        &BTreeMap::from([("TERM".to_string(), "vt100".to_string())]),
        None,
    );
    assert_eq!(spec.binary, PathBuf::from(DEFAULT_SHELL));
    assert!(spec.args.is_empty());
    let terms: Vec<_> = spec.env.iter().filter(|(k, _)| k == "TERM").collect();
    assert_eq!(terms, vec![&("TERM".to_string(), "vt100".to_string())]);
    Ok(())
}

fn reopen(resume_from: Option<u64>) -> Value {
    json!({ "op": "open", "cols": 80, "rows": 24, "resume_from": resume_from })
}

const PRINT_THREE: &str = "printf aaaa; sleep 0.4; printf bbbb; sleep 0.4; printf cccc; sleep 30";

fn data_seqs(sink: &Collect, id: &str) -> Vec<u64> {
    sink.output(id)
        .iter()
        .filter(|f| matches!(f, TerminalOutput::Data { .. }))
        .map(terminal_output_seq)
        .collect()
}

#[tokio::test]
async fn a_reattach_resumes_after_the_offset_the_reader_saw() -> TestResult {
    let f = fixture(&["l1"])?;
    let open = open(&["/bin/sh", "-c", PRINT_THREE]);
    f.terminals.handle(&request("l1", "t1", open)).await;
    f.sink
        .wait_until("all the output", |s| s.text("t1").contains("cccc"))
        .await?;
    let live = f.sink.output("t1").len();
    let seqs = data_seqs(&f.sink, "t1");
    assert_eq!(seqs.len(), 3, "{seqs:?}");

    // Seen everything: nothing is replayed.
    let seen_all = reopen(Some(seqs[2]));
    assert_eq!(f.terminals.handle(&request("l1", "t1", seen_all)).await, opened("t1"));
    assert_eq!(f.sink.output("t1").len(), live);

    // Seen the first frame: the next two come again, with their own seqs.
    let seen_one = reopen(Some(seqs[0]));
    f.terminals.handle(&request("l1", "t1", seen_one)).await;
    let replayed = f.sink.output("t1")[live..].to_vec();
    assert_eq!(
        replayed.iter().map(terminal_output_seq).collect::<Vec<_>>(),
        vec![seqs[1], seqs[2]]
    );
    assert!(replayed.iter().all(|f| terminal_output_bytes(f).is_some()));

    // No offset: everything the ring holds.
    let before = f.sink.output("t1").len();
    f.terminals.handle(&request("l1", "t1", reopen(None))).await;
    assert_eq!(f.sink.output("t1").len() - before, 3);
    f.terminals.handle(&request("l1", "t1", close())).await;
    Ok(())
}

#[tokio::test]
async fn a_reattach_older_than_the_ring_gets_a_resync_first() -> TestResult {
    // Room for one 4-byte frame only.
    let f = fixture_with_replay(&["l1"], 4)?;
    let open = open(&["/bin/sh", "-c", PRINT_THREE]);
    f.terminals.handle(&request("l1", "t1", open)).await;
    f.sink
        .wait_until("all the output", |s| s.text("t1").contains("cccc"))
        .await?;
    let live = f.sink.output("t1").len();
    let seqs = data_seqs(&f.sink, "t1");

    f.terminals.handle(&request("l1", "t1", reopen(Some(seqs[0])))).await;
    let replayed = f.sink.output("t1")[live..].to_vec();
    assert_eq!(replayed.len(), 2, "{replayed:?}");
    assert_eq!(replayed[0], TerminalOutput::Resync { from_seq: seqs[2] });
    assert_eq!(terminal_output_seq(&replayed[1]), seqs[2]);
    assert_eq!(terminal_output_bytes(&replayed[1]).ok_or("a data frame")?, b"cccc");
    f.terminals.handle(&request("l1", "t1", close())).await;
    Ok(())
}

#[tokio::test]
async fn with_replay_off_a_reattach_that_missed_output_must_resync() -> TestResult {
    let f = fixture_with_replay(&["l1"], 0)?;
    let open = open(&["/bin/sh", "-c", PRINT_THREE]);
    f.terminals.handle(&request("l1", "t1", open)).await;
    f.sink
        .wait_until("all the output", |s| s.text("t1").contains("cccc"))
        .await?;
    let live = f.sink.output("t1").len();
    let seqs = data_seqs(&f.sink, "t1");
    f.terminals.handle(&request("l1", "t1", reopen(Some(seqs[0])))).await;
    assert_eq!(
        f.sink.output("t1")[live..].to_vec(),
        vec![TerminalOutput::Resync { from_seq: seqs[2] + 1 }]
    );
    // A reader that saw it all has nothing to resync.
    f.terminals.handle(&request("l1", "t1", reopen(Some(seqs[2])))).await;
    assert_eq!(f.sink.output("t1").len(), live + 1);
    f.terminals.handle(&request("l1", "t1", close())).await;
    Ok(())
}
