#![cfg(any(target_os = "linux", target_os = "macos"))]

mod command_support;
mod support;

#[path = "command_capture_interruption_support/kill_races.rs"]
mod kill_races_tests;

use std::collections::{BTreeMap, HashMap};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use command_support::native::Directory;
use command_support::{
    Boxed, CMD, Reply, Request, Server, TestResult, body_of, completed, error_reply, exit_record, is_read,
    kill_request, locked, log_reply, polled, run_request, sandbox, started, terminal, with_env,
};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    KillCommandResponse, PollCommandResponse, RunCommandRequest, RunCommandResponse, UnixSignal,
};
use trogon_sandbox_vercel_worker::{LeaseSandbox, VercelExec};

const EMITTER: &str = "interrupted-native-emitter";
const OUT: &[u8] = b"before deadline\0\xff\xfe tail";
const ERR: &[u8] = b"before error\0\x80\xff tail";
const LEASE: &str = "interrupted-lease";
const ORIGINAL: &str = "original-command";
const CMD_PREFIX: &str = "/v2/sandboxes/sessions/session/cmd/";
const EMIT: &str = "printf 'before deadline\\000\\377\\376 tail'; printf 'before error\\000\\200\\377 tail' >&2";

fn pause(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

fn session_of(pid: i32) -> i32 {
    // SAFETY: getsid only reads kernel state for the pid.
    unsafe { libc::getsid(pid) }
}

fn signal(pid: i32, signal: i32) {
    // SAFETY: a signal sent to a pid this fixture owns the session of.
    unsafe {
        libc::kill(pid, signal);
    }
}

/// The processes of a private session, found with `ps` because GNU timeout moves its child to another group.
fn session_members(session: i32, with_zombies: bool) -> Result<Vec<i32>, Boxed> {
    let output = Command::new("ps").args(["-axo", "pid=,state="]).output()?;
    let text = String::from_utf8(output.stdout)?;
    let mut members: Vec<i32> = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<i32>().ok()?;
            (with_zombies || !fields.next()?.starts_with('Z')).then_some(pid)
        })
        .filter(|pid| session_of(*pid) == session)
        .collect();
    members.sort_unstable();
    Ok(members)
}

fn files_named(root: &Path, name: &str) -> Result<Vec<PathBuf>, Boxed> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            found.extend(files_named(&entry.path(), name)?);
        } else if kind.is_file() && entry.file_name() == name {
            found.push(entry.path());
        }
    }
    Ok(found)
}

fn spool_has(root: &Path, name: &str, bytes: &[u8]) -> Result<bool, Boxed> {
    Ok(files_named(root, name)?
        .iter()
        .any(|path| std::fs::read(path).is_ok_and(|content| content == bytes)))
}

fn spools_cleaned(root: &Path) -> Result<bool, Boxed> {
    Ok(files_named(root, "out")?.is_empty() && files_named(root, "err")?.is_empty())
}

/// The capture directory the guest script created for the command bytes.
fn spool_dir(root: &Path) -> Result<PathBuf, Boxed> {
    let out = files_named(root, "out")?
        .into_iter()
        .find(|path| std::fs::read(path).is_ok_and(|content| content == OUT))
        .ok_or("no capture directory holds the command bytes")?;
    Ok(out.parent().ok_or("the spool has no directory")?.to_owned())
}

struct NativeCommand {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
    exit: Option<i32>,
    capture_id: Option<String>,
}

impl NativeCommand {
    fn try_status(&mut self) -> std::io::Result<Option<i32>> {
        if self.exit.is_none()
            && let Some(status) = self.child.try_wait()?
        {
            self.exit = Some(
                status
                    .code()
                    .or_else(|| status.signal().map(|signal| 128 + signal))
                    .unwrap_or(-1),
            );
        }
        Ok(self.exit)
    }

    fn status(&mut self) -> Option<i32> {
        self.try_status().ok().flatten()
    }

    fn await_exit(&mut self, limit: Duration) -> Result<i32, Boxed> {
        let until = Instant::now() + limit;
        while self.try_status()?.is_none() && Instant::now() < until {
            pause(5);
        }
        Ok(self.exit.ok_or("the native command did not exit in time")?)
    }

    /// Stops every member of the private session until the set settles, then kills them all.
    fn kill_session(&mut self) -> Result<(), Boxed> {
        let session = i32::try_from(self.child.id())?;
        let mut stopped = Vec::new();
        for _ in 0..20 {
            let members = session_members(session, true)?;
            members.iter().for_each(|pid| signal(*pid, libc::SIGSTOP));
            if members == stopped {
                break;
            }
            stopped = members;
            pause(5);
        }
        session_members(session, true)?
            .iter()
            .for_each(|pid| signal(*pid, libc::SIGKILL));
        let exit = self.await_exit(Duration::from_secs(2))?;
        (exit == 137)
            .then_some(())
            .ok_or("native SIGKILL had an unexpected exit")?;
        Ok(())
    }

    /// Waits for the helpers a finished command left in its session to end.
    fn settle_finished_session(&self) -> Result<(), Boxed> {
        let session = i32::try_from(self.child.id())?;
        let mut left = 600;
        while !session_members(session, false)?.is_empty() && left > 0 {
            left -= 1;
            pause(5);
        }
        session_members(session, false)?
            .is_empty()
            .then_some(())
            .ok_or("a finished session kept live helper children")?;
        Ok(())
    }
}

impl Drop for NativeCommand {
    fn drop(&mut self) {
        let _ = self.try_status().ok().flatten().is_none().then(|| self.kill_session());
    }
}

#[derive(Debug, Clone, Copy, Default)]
enum NativeView {
    #[default]
    Exact,
    Missing,
    Nonterminal,
    Foreign,
}

#[derive(Default)]
struct Native {
    commands: HashMap<String, NativeCommand>,
    user_starts: usize,
    controls: usize,
    kills: usize,
    reject_kills: bool,
    reject_controls: bool,
    reject_cleanup: bool,
    lost_backing_logs: bool,
    malformed_backing_logs: bool,
    foreign_backing_logs: bool,
    kill_404_after_terminal: bool,
    kill_204: bool,
    kill_replies: Vec<u16>,
    accepted_kills: usize,
    terminal_owner_valid: bool,
    saved_status: Option<String>,
    timedout_marker: bool,
    view: NativeView,
    status_reads: usize,
    reject_original_logs: bool,
    spooled_before_kill: bool,
    native_budgets: Vec<Option<u64>>,
    noisy_stderr: bool,
    noisy_cleanup: bool,
    cleanups: Vec<String>,
}

impl Native {
    fn original(&mut self) -> Result<&mut NativeCommand, Boxed> {
        Ok(self
            .commands
            .get_mut(ORIGINAL)
            .ok_or("the original command was not started")?)
    }

    /// Records what the guest script left in its capture directory once the carrier ended.
    fn observe_spool(&mut self, root: &Path) -> Result<(), Boxed> {
        let capture_id = self
            .original()?
            .capture_id
            .clone()
            .ok_or("the launch carried no capture id")?;
        let directory = spool_dir(root)?;
        self.terminal_owner_valid = std::fs::read(directory.join("owner"))? == format!("{capture_id}\n").as_bytes();
        self.saved_status = std::fs::read_to_string(directory.join("status"))
            .ok()
            .map(|value| value.trim().to_owned());
        self.timedout_marker = directory.join("timedout").is_file();
        Ok(())
    }

    fn timeout_proof(&self) -> bool {
        self.terminal_owner_valid && self.saved_status.as_deref() == Some("timeout") && self.timedout_marker
    }
}

fn spawn(directory: &Directory, body: &Value, id: &str) -> Result<NativeCommand, Boxed> {
    let (mut command, stdout, stderr) = directory.command(body, id)?;
    // SAFETY: setsid is async-signal-safe and the closure touches no other state. A failed spawn reports it, so the
    // child id is a proven private session id.
    unsafe {
        command.pre_exec(|| {
            // SAFETY: see above.
            (libc::setsid() >= 0)
                .then_some(())
                .ok_or_else(std::io::Error::last_os_error)
        });
    }
    let capture_id = body
        .pointer("/args/1")
        .and_then(Value::as_str)
        .and_then(|script| script.strip_prefix("set -- "))
        .and_then(|rest| rest.split_whitespace().next())
        .map(str::to_owned);
    Ok(NativeCommand {
        child: command.spawn()?,
        stdout,
        stderr,
        exit: None,
        capture_id,
    })
}

struct Shared {
    directory: Directory,
    state: Mutex<Native>,
}

impl Shared {
    fn respond(&self, request: &Request) -> Result<Reply, Boxed> {
        let route = request.route();
        if request.method == "POST" && route == CMD {
            return self.launch(&body_of(request));
        }
        if request.method == "POST" && route.ends_with("/original-command/kill") {
            return self.kill(request);
        }
        let suffix = route.strip_prefix(CMD_PREFIX).unwrap_or_default();
        let (id, logs) = suffix.strip_suffix("/logs").map_or((suffix, false), |id| (id, true));
        let mut state = locked(&self.state);
        if id == ORIGINAL && logs && state.reject_original_logs {
            return Ok(error_reply(503, "injected original log failure"));
        }
        if id == ORIGINAL && !logs {
            state.status_reads += 1;
            match state.view {
                NativeView::Missing => return Ok(error_reply(404, "injected missing native receipt")),
                NativeView::Nonterminal => return Ok(Reply::json(200, exit_record(id, None::<i32>))),
                NativeView::Foreign => return Ok(Reply::json(200, exit_record("foreign-command", 0))),
                NativeView::Exact => {}
            }
        }
        let backing = logs && id.starts_with("control-");
        if backing && state.lost_backing_logs {
            return Ok(error_reply(404, "injected backing log loss"));
        }
        if backing && state.malformed_backing_logs {
            return Ok(log_reply("incomplete backing frame"));
        }
        let foreign = backing && state.foreign_backing_logs;
        let noise = if backing && state.noisy_stderr {
            "{\"stream\":\"stderr\",\"data\":\"x\"}\n"
        } else if state.noisy_cleanup && state.cleanups.iter().any(|cleanup| cleanup == id) {
            "{\"stream\":\"stdout\",\"data\":\"x\"}\n"
        } else {
            ""
        };
        let Some(command) = state.commands.get_mut(id) else {
            return Ok(error_reply(404, "unknown native fixture operation"));
        };
        if !logs {
            return Ok(Reply::json(200, exit_record(id, command.status())));
        }
        let stdout = json!({"stream": "stdout", "data": String::from_utf8_lossy(&std::fs::read(&command.stdout)?)});
        let stderr = json!({"stream": "stderr", "data": String::from_utf8_lossy(&std::fs::read(&command.stderr)?)});
        let identity = if foreign {
            "{\"command\":{\"id\":\"another-command\",\"exitCode\":0}}\n"
        } else {
            ""
        };
        Ok(Reply::Complete(
            200,
            format!("{identity}{stdout}\n{stderr}\n{noise}").into_bytes(),
        ))
    }

    fn launch(&self, body: &Value) -> Result<Reply, Boxed> {
        let user = body["args"]
            .as_array()
            .is_some_and(|args| args.iter().any(|arg| arg == EMITTER));
        let mut state = locked(&self.state);
        let id = if user {
            state.user_starts += 1;
            state.native_budgets.push(body["timeout"].as_u64());
            ORIGINAL.to_owned()
        } else {
            // A read or cleanup control must not extend the original execution.
            state
                .original()?
                .status()
                .ok_or("a control ran while the original command was alive")?;
            state.controls += 1;
            if state.reject_controls || (state.reject_cleanup && !is_read(body)) {
                return Ok(error_reply(503, "injected capture control failure"));
            }
            let id = format!("control-{}", state.controls);
            if !is_read(body) {
                state.cleanups.push(id.clone());
            }
            id
        };
        let mut command = spawn(&self.directory, body, &id)?;
        let exit = if user {
            None
        } else {
            Some(command.await_exit(Duration::from_secs(5))?)
        };
        state.commands.insert(id.clone(), command);
        let record = exit_record(&id, exit);
        Ok(if user && body["wait"] == true {
            Reply::Held(format!("{record}\n").into_bytes())
        } else {
            terminal(body, &record)
        })
    }

    fn kill(&self, request: &Request) -> Result<Reply, Boxed> {
        let mut state = locked(&self.state);
        state.kills += 1;
        let root = &self.directory.path;
        state.spooled_before_kill = spool_has(root, "out", OUT)? && spool_has(root, "err", ERR)?;
        if state.kill_404_after_terminal {
            state.original()?.await_exit(Duration::from_secs(3))?;
            state.observe_spool(root)?;
            state.kill_replies.push(404);
            return Ok(error_reply(404, "command not found or has exited"));
        }
        if state.reject_kills {
            state.kill_replies.push(503);
            return Ok(error_reply(503, "injected native kill refusal"));
        }
        assert_eq!(body_of(request)["signal"], 9);
        state.original()?.kill_session()?;
        state.accepted_kills += 1;
        let status = if state.kill_204 { 204 } else { 200 };
        state.kill_replies.push(status);
        Ok(if state.kill_204 {
            Reply::Complete(204, Vec::new())
        } else {
            Reply::json(200, json!({}))
        })
    }
}

#[derive(Debug)]
struct Evidence {
    starts: usize,
    kills: usize,
    controls: usize,
    accepted_kills: usize,
    replies: Vec<u16>,
    budgets: Vec<Option<u64>>,
    native_exit: Option<i32>,
    spooled: bool,
    owner_valid: bool,
    status: Option<String>,
    timeout_proof: bool,
}

struct Fixture {
    server: Server,
    shared: Arc<Shared>,
    exec: VercelExec,
    sandbox: LeaseSandbox,
}

impl Fixture {
    async fn new(reject_kills: bool) -> Result<Self, Boxed> {
        let directory = Directory::new("vercel-interruption-", &["base64", "mktemp"])?;
        std::fs::write(directory.path.join("unrelated.keep"), b"unrelated file")?;
        let shared = Arc::new(Shared {
            directory,
            state: Mutex::new(Native {
                reject_kills,
                ..Native::default()
            }),
        });
        let state = shared.clone();
        let server = Server::start(move |request| command_support::fixture_reply(state.respond(&request))).await?;
        let exec = server.exec()?;
        Ok(Self {
            server,
            shared,
            exec,
            sandbox: sandbox(),
        })
    }

    fn root(&self) -> &Path {
        &self.shared.directory.path
    }

    fn state(&self) -> MutexGuard<'_, Native> {
        locked(&self.shared.state)
    }

    fn calls(&self) -> (usize, usize) {
        let native = self.state();
        (native.status_reads, native.controls)
    }

    fn tweak(&self, change: impl FnOnce(&mut Native)) {
        change(&mut self.state());
    }

    fn request(&self, script: &str, detached: bool) -> Result<RunCommandRequest, Boxed> {
        let command = json!(["sh", "-c", script, EMITTER]);
        Ok(run_request(
            LEASE,
            command,
            json!({"cwd": self.root(), "timeout_seconds": 1, "detached": detached}),
        )?)
    }

    fn interrupted(&self) -> Result<RunCommandRequest, Boxed> {
        self.request(&format!("trap '' TERM; {EMIT}; sleep 60"), false)
    }

    async fn run(&self, request: &RunCommandRequest) -> RunCommandResponse {
        self.exec.handle_run_command(&self.sandbox, request).await
    }

    async fn poll(&self) -> PollCommandResponse {
        self.exec.handle_poll_command(&self.sandbox, LEASE, ORIGINAL).await
    }

    async fn poll_as(&self, lease: &str, other_session: bool) -> PollCommandResponse {
        let other = LeaseSandbox {
            name: "other".into(),
            session_id: "other-session".into(),
            routes: BTreeMap::new(),
        };
        let sandbox = if other_session { &other } else { &self.sandbox };
        self.exec.handle_poll_command(sandbox, lease, ORIGINAL).await
    }

    async fn kill(&self) -> KillCommandResponse {
        let request = kill_request(LEASE, ORIGINAL, UnixSignal::Kill);
        self.exec.handle_kill_command(&self.sandbox, LEASE, &request).await
    }

    fn evidence(&self) -> Result<Evidence, Boxed> {
        let mut native = self.state();
        let native_exit = native.original()?.status();
        Ok(Evidence {
            starts: native.user_starts,
            kills: native.kills,
            controls: native.controls,
            accepted_kills: native.accepted_kills,
            replies: native.kill_replies.clone(),
            budgets: native.native_budgets.clone(),
            native_exit,
            spooled: native.spooled_before_kill,
            owner_valid: native.terminal_owner_valid,
            status: native.saved_status.clone(),
            timeout_proof: native.timeout_proof(),
        })
    }

    /// Kills the original command from outside the worker, as the guest would.
    async fn external_kill(&self) -> TestResult {
        let root = self.root();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !(spool_has(root, "out", OUT)? && spool_has(root, "err", ERR)?) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok::<(), Boxed>(())
        })
        .await??;
        let mut native = self.state();
        native.spooled_before_kill = true;
        native.original()?.kill_session()?;
        native.observe_spool(root)?;
        Ok(())
    }

    async fn shutdown(self) -> TestResult {
        self.server.shutdown().await;
        let mut native = locked(&self.shared.state);
        let settle = native.kill_404_after_terminal;
        for command in native.commands.values_mut() {
            match command.status() {
                Some(_) if settle => command.settle_finished_session()?,
                Some(_) => {}
                None => command.kill_session()?,
            }
        }
        native.commands.clear();
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_native_deadline_recovers_real_binary_spools_after_carrier_interruption() -> TestResult {
    let fixture = Fixture::new(false).await?;
    let response = fixture.run(&fixture.interrupted()?).await;
    let observed = fixture.evidence()?;
    let carrier_empty = {
        let mut native = fixture.state();
        let original = native.original()?;
        std::fs::read(&original.stdout)?.is_empty() && std::fs::read(&original.stderr)?.is_empty()
    };
    let spool_cleaned = spools_cleaned(fixture.root())?;
    let unrelated = std::fs::read(fixture.root().join("unrelated.keep"))?;
    fixture.shutdown().await?;
    assert!(
        observed.spooled,
        "the bytes must reach the private spools before the interruption"
    );
    assert_eq!((observed.starts, observed.kills), (1, 1));
    assert_eq!(observed.budgets, [Some(1000)]);
    assert!(carrier_empty);
    assert_eq!(observed.native_exit, Some(137));
    let output = completed(response)?;
    assert!(output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert_eq!(output.exit_code, -1);
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, ERR);
    assert!(spool_cleaned, "a completed recovery removes its owned spools");
    assert_eq!(unrelated, b"unrelated file");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_kill_after_a_refused_deadline_recovers_real_bytes_without_inventing_timeout() -> TestResult {
    let fixture = Fixture::new(true).await?;
    let response = fixture.run(&fixture.interrupted()?).await;
    let running = fixture.poll().await;
    let foreign = fixture.poll_as("foreign-lease", false).await;
    fixture.tweak(|native| native.reject_kills = false);
    let signaled = fixture.kill().await;
    let recovered = fixture.poll().await;
    let repeated = fixture.poll().await;
    let late_kill = fixture.kill().await;
    let observed = fixture.evidence()?;
    let spool_cleaned = spools_cleaned(fixture.root())?;
    let unrelated = std::fs::read(fixture.root().join("unrelated.keep"))?;
    fixture.shutdown().await?;
    assert!(observed.spooled);
    assert!(matches!(response, RunCommandResponse::Denied(_)));
    assert!(matches!(running, PollCommandResponse::Running));
    assert!(matches!(foreign, PollCommandResponse::NotFound));
    assert_eq!(signaled, KillCommandResponse::Signaled);
    assert_eq!((observed.starts, observed.kills), (1, 2));
    assert_eq!(observed.budgets, [Some(1000)]);
    assert_eq!(observed.native_exit, Some(137));
    let output = polled(recovered.clone())?;
    assert!(!output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert_eq!(output.exit_code, 137);
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, ERR);
    assert!(spool_cleaned);
    assert_eq!(unrelated, b"unrelated file");
    assert_eq!(repeated, recovered, "the retained identity replays the same completion");
    assert_eq!(late_kill, KillCommandResponse::AlreadyCompleted);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_authority_survives_missing_wrong_markers_read_failure_and_cleanup_retry() -> TestResult {
    let fixture = Fixture::new(true).await?;
    let user_tmp = fixture.root().join("user-controlled-tmp");
    std::fs::create_dir(&user_tmp)?;
    let request = with_env(fixture.interrupted()?, "TMPDIR", &user_tmp.to_string_lossy())?;
    let response = fixture.run(&request).await;
    fixture.tweak(|native| native.reject_kills = false);
    let signaled = fixture.kill().await;
    let directory = spool_dir(fixture.root())?;
    assert!(
        !directory.starts_with(&user_tmp),
        "user env must not relocate control capture storage"
    );
    let spool = directory.join("out");
    let owner = directory.join("owner");
    let owner_bytes = std::fs::read(&owner)?;
    let controls_before = fixture.state().controls;
    let foreign = fixture.poll_as("foreign-lease", false).await;
    let foreign_session = fixture.poll_as(LEASE, true).await;
    assert_eq!(fixture.state().controls, controls_before);

    std::fs::rename(&owner, directory.join("owner.saved"))?;
    let missing = fixture.poll().await;
    std::fs::rename(directory.join("owner.saved"), &owner)?;
    std::fs::write(&owner, b"wrong-owner\n")?;
    let wrong = fixture.poll().await;
    std::fs::write(&owner, owner_bytes)?;
    fixture.tweak(|native| native.reject_controls = true);
    let unreadable = fixture.poll().await;
    fixture.tweak(|native| native.reject_controls = false);
    assert_eq!(std::fs::read(&spool)?, OUT);
    assert_eq!(std::fs::read(directory.join("err"))?, ERR);

    fixture.tweak(|native| native.reject_cleanup = true);
    let cleanup_failed = fixture.poll().await;
    std::fs::write(&spool, b"changed after decoded completion")?;
    std::fs::write(directory.join("err"), b"changed after decoded completion")?;
    fixture.tweak(|native| native.reject_cleanup = false);
    let recovered = fixture.poll().await;
    let controls_after = fixture.state().controls;
    let repeated = fixture.poll().await;
    let observed = fixture.evidence()?;
    let spool_cleaned = !directory.exists();
    let unrelated = std::fs::read(fixture.root().join("unrelated.keep"))?;
    fixture.shutdown().await?;
    assert!(matches!(response, RunCommandResponse::Denied(_)));
    assert_eq!(signaled, KillCommandResponse::Signaled);
    assert!(matches!(foreign, PollCommandResponse::NotFound));
    assert!(matches!(foreign_session, PollCommandResponse::NotFound));
    for response in [missing, wrong, unreadable, cleanup_failed] {
        assert!(
            matches!(response, PollCommandResponse::Denied(_)),
            "unproved output must retain authority: {response:?}"
        );
    }
    let output = polled(recovered.clone())?;
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, ERR);
    assert_eq!(output.exit_code, 137);
    assert!(!output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert_eq!(repeated, recovered);
    assert_eq!(observed.starts, 1);
    assert_eq!(observed.budgets, [Some(1000)]);
    assert_eq!(
        observed.controls, controls_after,
        "a settled completion replays without another native control"
    );
    assert!(spool_cleaned);
    assert_eq!(unrelated, b"unrelated file");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_capture_retains_its_source_when_backing_logs_are_lost_malformed_or_misattributed() -> TestResult {
    let fixture = Fixture::new(true).await?;
    let initial = fixture.run(&fixture.interrupted()?).await;
    fixture.tweak(|native| native.reject_kills = false);
    let signaled = fixture.kill().await;
    let completed = fixture.poll().await;
    let spool_cleaned = spools_cleaned(fixture.root())?;
    let controls_before = fixture.state().controls;
    fixture.tweak(|native| native.lost_backing_logs = true);
    let lost = fixture.poll().await;
    fixture.tweak(|native| (native.lost_backing_logs, native.malformed_backing_logs) = (false, true));
    let malformed = fixture.poll().await;
    fixture.tweak(|native| (native.malformed_backing_logs, native.foreign_backing_logs) = (false, true));
    let foreign = fixture.poll().await;
    fixture.tweak(|native| native.foreign_backing_logs = false);
    let restored = fixture.poll().await;
    let observed = fixture.evidence()?;
    fixture.shutdown().await?;
    assert!(matches!(initial, RunCommandResponse::Denied(_)));
    assert_eq!(signaled, KillCommandResponse::Signaled);
    let output = polled(completed.clone())?;
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, ERR);
    assert_eq!(output.exit_code, 137);
    assert!(!output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert!(spool_cleaned);
    for response in [lost, malformed, foreign] {
        assert!(
            matches!(response, PollCommandResponse::Denied(_)),
            "untrusted backing logs fail closed: {response:?}"
        );
    }
    assert_eq!(
        restored, completed,
        "the original provider source recovers without raw spools"
    );
    assert_eq!(observed.starts, 1);
    assert_eq!(
        observed.controls, controls_before,
        "a replay launches no user, encoder or cleanup control"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_controls_with_unexpected_output_retain_the_recovery_authority() -> TestResult {
    let fixture = Fixture::new(true).await?;
    let initial = fixture.run(&fixture.interrupted()?).await;
    fixture.tweak(|native| native.reject_kills = false);
    let signaled = fixture.kill().await;
    fixture.tweak(|native| native.noisy_stderr = true);
    let noisy_read = fixture.poll().await;
    fixture.tweak(|native| (native.noisy_stderr, native.noisy_cleanup) = (false, true));
    let noisy_cleanup = fixture.poll().await;
    fixture.tweak(|native| native.noisy_cleanup = false);
    let recovered = fixture.poll().await;
    fixture.shutdown().await?;
    assert!(matches!(initial, RunCommandResponse::Denied(_)));
    assert_eq!(signaled, KillCommandResponse::Signaled);
    assert!(matches!(noisy_read, PollCommandResponse::Denied(_)));
    assert!(matches!(noisy_cleanup, PollCommandResponse::Denied(_)));
    assert_eq!(polled(recovered)?.stdout, OUT);
    Ok(())
}
