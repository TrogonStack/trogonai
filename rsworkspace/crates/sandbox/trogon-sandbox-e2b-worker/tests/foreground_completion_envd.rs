#![cfg(target_os = "linux")]

mod launch_scope_support;
mod native_command_support;

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use launch_scope_support::plain;
use native_command_support::command_deadline_support::{
    Reply, Request, Server, TestResult, frames, lease_sandbox, run_request, started,
};
use native_command_support::{Binding, Boxed, Guest, NativeEnvd};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    KillCommandRequest, KillCommandResponse, LeaseError, LeaseId, PollCommandResponse, RunCommandRequest,
    RunCommandResponse, UnixSignal,
};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};
use trogon_std::env::{ReadEnv, SystemEnv};

const TARGET: &str = r#"d=$1; mode=$2
mkdir -p -- "$d"
printf 'target\n' >>"$d/launches"
identity() {
  IFS= read -r line <"/proc/$1/stat" || return 1
  p=${line%% *}; fields=${line##*) }; set -- $fields
  group=$3; session=$4; shift 19
  printf '%s %s %s %s\n' "$p" "$1" "$group" "$session"
}
identity $$ >"$d/target"
printf 'native stdout\n'
printf 'native stderr\n' >&2
[ "$mode" != control ] || exit 7
trap '' USR1
sleep 60 & child=$!
identity "$child" >"$d/child"
: >"$d/ready"
wait "$child"
"#;

const INVOKE: &str = r#"d=$1; shift
mkdir -p -- "$d"
setsid /bin/bash -c 'd=$1; shift
argv=("$@")
IFS= read -r line <"/proc/$$/stat" || exit 125
fields=${line##*) }; set -- $fields; shift 19
printf "%s %s\n" "$$" "$1" >"$d/binding"
exec "${argv[@]}"' native "$d" "$@" </dev/null >"$d/out" 2>"$d/err" &
p=$!
wait "$p"; code=$?
printf '%s\n' "$code" >"$d/exit.tmp"
mv "$d/exit.tmp" "$d/exit"
"#;

const KILL_EXACT: &str = r#"p=$1; expected=$2
IFS= read -r line <"/proc/$p/stat" || exit 1
fields=${line##*) }; set -- $fields; shift 19
[ "$1" = "$expected" ] || exit 1
kill -KILL "$p""#;

/// A check whose failure message is built only when it is needed.
fn require(holds: bool, message: impl Into<Boxed>) -> Result<(), Boxed> {
    holds.then_some(()).ok_or_else(|| message.into())
}

/// A native process: its id, its start time and the group and session it leads or belongs to.
#[derive(Clone, Copy, Debug)]
struct Identity {
    pid: u32,
    started: u64,
    group: u32,
    session: u32,
}

impl Identity {
    fn parse(bytes: &[u8]) -> Result<Self, Boxed> {
        let fields = std::str::from_utf8(bytes)?.split_whitespace().collect::<Vec<_>>();
        let [pid, started, group, session]: [&str; 4] =
            fields.try_into().map_err(|_| "incomplete native target identity")?;
        Ok(Self {
            pid: pid.parse()?,
            started: started.parse()?,
            group: group.parse()?,
            session: session.parse()?,
        })
    }
}

fn first_pid(binding: &[u8]) -> Result<u32, Boxed> {
    Ok(std::str::from_utf8(binding)?
        .split_whitespace()
        .next()
        .ok_or("native binding is empty")?
        .parse()?)
}

fn read(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn identity(path: &str) -> Result<Option<Identity>, Boxed> {
    read(path).as_deref().map(Identity::parse).transpose()
}

fn target_launches(root: &str) -> Result<usize, Boxed> {
    let bytes = read(&format!("{root}/launches")).ok_or("the actual target never launched")?;
    Ok(std::str::from_utf8(&bytes)?.lines().count())
}

/// The group leader of the target: the wrapper that owns the group the command runs in.
/// A leader that already ended has no start time to compare, and then owns no live members.
fn leader(target: Identity) -> Binding {
    let started = std::fs::read_to_string(format!("/proc/{}/stat", target.group))
        .ok()
        .and_then(|stat| stat.rsplit(") ").next()?.split_whitespace().nth(19)?.parse().ok())
        .unwrap_or_default();
    Binding {
        pid: target.group,
        started,
    }
}

fn members(guest: &Guest, target: Identity) -> Result<Vec<u32>, Boxed> {
    guest.group_members(leader(target))
}

/// Waits until the target and its child are live in the group the command runs in.
fn wait_ready(guest: &Guest, root: &str) -> Result<(Identity, Identity), Boxed> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if read(&format!("{root}/ready")).is_some() {
            let target = identity(&format!("{root}/target"))?.ok_or("target identity missing")?;
            let child = identity(&format!("{root}/child"))?.ok_or("child identity missing")?;
            let running = members(guest, target)?;
            let both_live = running.contains(&target.pid) && running.contains(&child.pid);
            require(both_live, "the actual target and child were not both ready")?;
            let same_group = child.group == target.group && child.session == target.session;
            require(same_group, "descendant escaped the original target group")?;
            return Ok((target, child));
        }
        require(Instant::now() < until, "native target readiness timed out")?;
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn reply(pid: u32, stdout: &[u8], stderr: &[u8], code: Option<i32>, fault: Option<&str>) -> Reply {
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut events = vec![
        (0, json!({"event":{"start":{"pid":pid}}})),
        (0, json!({"event":{"data":{"stdout":encode(stdout)}}})),
        (0, json!({"event":{"data":{"stderr":encode(stderr)}}})),
    ];
    if let Some(code) = code {
        let status = if code == -1 {
            "signal: killed".to_owned()
        } else {
            format!("exit status {code}")
        };
        let mut end = json!({"exited":code >= 0,"exitCode":code,"status":status});
        if code != 0 {
            end["error"] = json!(status);
        }
        events.push((0, json!({"event":{"end":end}})));
    }
    events.push((
        2,
        fault.map_or_else(
            || json!({}),
            |code| json!({"error":{"code":code,"message":"request ended before exact execution settlement"}}),
        ),
    ));
    Reply::Complete(200, frames(&events))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    Canceled,
    Deadline,
    MainEnd,
    Natural,
}

struct Fixture {
    guest: Arc<Guest>,
    root: String,
    scenario: Scenario,
    rpc_starts: AtomicUsize,
    injected: AtomicBool,
}

impl Fixture {
    /// The process id of the foreground launch the fault applies to, once the target is ready.
    fn armed_pid(&self, nth: usize, dir: &str) -> Result<Option<u32>, Boxed> {
        let ready = self.scenario != Scenario::Natural
            && nth == 0
            && !self.injected.load(Ordering::SeqCst)
            && read(&format!("{}/ready", self.root)).is_some();
        let target = identity(&format!("{}/target", self.root))?.filter(|_| ready);
        let binding = read(&format!("{dir}/binding")).filter(|_| ready);
        let pid = binding.as_deref().map(first_pid).transpose()?;
        Ok(match (pid, target) {
            (Some(pid), Some(target)) => (!members(&self.guest, target)?.is_empty()).then_some(pid),
            _ => None,
        })
    }

    fn fault_target(&self, nth: usize, dir: &str) -> Result<Option<(Identity, Identity, u32)>, Boxed> {
        self.armed_pid(nth, dir)?
            .map(|pid| wait_ready(&self.guest, &self.root).map(|(target, child)| (target, child, pid)))
            .transpose()
    }

    /// Starts the requested process in its own session and process group, as the sandbox would.
    fn invoke(&self, request: &Request, dir: &str) -> Result<(), Boxed> {
        let frame = request.frame();
        let process = &frame["process"];
        let mut command = Command::new("bash");
        command.args([
            "-c",
            INVOKE,
            "native",
            dir,
            process["cmd"].as_str().ok_or("process has no command")?,
        ]);
        command.args(request.args()).env_clear();
        if let Some(path) = SystemEnv.var_os("PATH") {
            command.env("PATH", path);
        }
        let envs: BTreeMap<String, String> = serde_json::from_value(process["envs"].clone())?;
        command.envs(&envs);
        let mut child = command
            .current_dir(self.guest.path(""))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn handle(&self, request: Request) -> Result<Reply, Boxed> {
        if request.method == "GET" && request.path.starts_with("/files?") {
            let path = request.query("path").ok_or("missing native file path")?;
            return Ok(Reply::Complete(200, read(&path).unwrap_or_default()));
        }
        let start = request.method == "POST" && request.path == "/process.Process/Start";
        require(start, "unexpected native fixture endpoint")?;
        let nth = self.rpc_starts.fetch_add(1, Ordering::SeqCst);
        let dir = format!("{}/rpc-{nth}", self.root);
        self.invoke(&request, &dir)?;
        let until = Instant::now() + Duration::from_secs(6);
        let mut forced_end = false;
        loop {
            if let Some((target, child, pid)) = self.fault_target(nth, &dir)? {
                if self.scenario != Scenario::MainEnd {
                    if self.scenario == Scenario::Deadline {
                        std::thread::sleep(Duration::from_secs(2));
                    }
                    self.injected.store(true, Ordering::SeqCst);
                    let out = read(&format!("{dir}/out")).unwrap_or_default();
                    let err = read(&format!("{dir}/err")).unwrap_or_default();
                    let fault = if self.scenario == Scenario::Canceled {
                        "canceled"
                    } else {
                        "deadline_exceeded"
                    };
                    return Ok(reply(pid, &out, &err, None, Some(fault)));
                }
                let killed = Command::new("bash")
                    .args([
                        "-c",
                        KILL_EXACT,
                        "kill-exact",
                        &target.pid.to_string(),
                        &target.started.to_string(),
                    ])
                    .output()?;
                require(killed.status.success(), "the exact main kill failed")?;
                let remaining = members(&self.guest, target)?;
                let survived = remaining.contains(&child.pid);
                require(survived, "native child did not survive main-only kill")?;
                self.injected.store(true, Ordering::SeqCst);
                forced_end = true;
            }
            // The foreground wrapper stays open until the scheduled fault reaches it.
            let settled = nth != 0 || self.scenario == Scenario::Natural || forced_end;
            if let Some(code) = read(&format!("{dir}/exit")).filter(|_| settled) {
                let code: i32 = std::str::from_utf8(&code)?.trim().parse()?;
                let pid = first_pid(&read(&format!("{dir}/binding")).ok_or("native binding missing")?)?;
                let out = read(&format!("{dir}/out")).unwrap_or_default();
                let err = read(&format!("{dir}/err")).unwrap_or_default();
                return Ok(reply(
                    pid,
                    &out,
                    &err,
                    Some(if forced_end { -1 } else { code }),
                    forced_end.then_some("deadline_exceeded"),
                ));
            }
            let in_budget = Instant::now() < until;
            require(in_budget, "native RPC did not settle within fixture budget")?;
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn client(url: &str) -> TestResult<(E2bExec, trogon_sandbox_e2b_worker::LeaseSandbox)> {
    Ok((
        E2bExec::new(Envd::new(Duration::from_secs(10))?),
        lease_sandbox(url, "owned-native-foreground")?,
    ))
}

fn request(root: &str, control: bool, detached: bool) -> TestResult<RunCommandRequest> {
    run_request(json!({
        "lease_id":"native-completion",
        "command":["bash","-c",TARGET,"target",root,if control {"control"} else {"hold"}],
        "run_as_user":"root", "detached":detached, "timeout_seconds":2,
        "deadline":if detached { "lease" } else { "timeout" },
        "env":if detached { json!({"BASH_ENV":format!("{root}/ignore-usr1.sh")}) } else { json!({}) },
    }))
}

/// Records the target the command left behind, so the guest's cleanup removes it.
fn adopt(guest: &Guest, root: &str) -> TestResult {
    let targets = plain(identity(&format!("{root}/target")))?
        .map(leader)
        .into_iter()
        .collect::<Vec<_>>();
    guest
        .bindings
        .lock()
        .map_err(|_| "the binding record is poisoned")?
        .extend(targets);
    Ok(())
}

async fn foreground(scenario: Scenario) -> TestResult {
    let guest = plain(Guest::create())?;
    let root = guest.path("native-completion").to_string_lossy().into_owned();
    let state = Arc::new(Fixture {
        guest: Arc::clone(&guest),
        root: root.clone(),
        scenario,
        rpc_starts: AtomicUsize::new(0),
        injected: AtomicBool::new(false),
    });
    let capture = Arc::clone(&state);
    let server = Server::start(move |request| {
        capture
            .handle(request)
            .unwrap_or_else(|error| Reply::json(503, Value::from(error.to_string())))
    })
    .await?;
    let observed: Result<(), Boxed> = async {
        let (exec, sandbox) = client(&server.url).map_err(|error| error.to_string())?;
        let body = request(&root, scenario == Scenario::Natural, false).map_err(|error| error.to_string())?;
        let response = tokio::time::timeout(Duration::from_secs(15), exec.handle_run_command(&sandbox, &body)).await?;
        let target = identity(&format!("{root}/target"))?.ok_or("actual target identity missing")?;
        let remaining = members(&guest, target)?;
        let launches = target_launches(&root)?;
        require(launches == 1, "the actual target must launch exactly once")?;
        if scenario == Scenario::Natural {
            require(remaining.is_empty(), "natural native group remains live")?;
            let preserved = matches!(&response, RunCommandResponse::Completed(out)
                if out.exit_code == 7 && !out.timed_out.is_timed_out() && !out.truncated.is_truncated()
                    && out.stdout == b"native stdout\n" && out.stderr == b"native stderr\n");
            require(preserved, "nonzero End must preserve output")?;
        } else {
            let until = Instant::now() + Duration::from_secs(3);
            let settled = loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let live = members(&guest, target)?;
                if live.is_empty() || Instant::now() >= until {
                    break live;
                }
            };
            let faulted_or_settled = state.injected.load(Ordering::SeqCst) || settled.is_empty();
            require(faulted_or_settled, "no fault or settlement observed")?;
            let refused = matches!(&response, RunCommandResponse::Denied(LeaseError::Backend { .. }));
            require(refused, "terminal completion cannot outrun native group settlement")?;
        }
        Ok(())
    }
    .await;
    server.shutdown().await;
    adopt(&guest, &root)?;
    plain(guest.cleanup())?;
    plain(observed)
}

async fn detached_signal(signal: UnixSignal) -> TestResult {
    let guest = plain(Guest::create())?;
    let root = guest.path("native-signal").to_string_lossy().into_owned();
    let envd = plain(NativeEnvd::start(Arc::clone(&guest)).await)?;
    let observed: Result<(), Boxed> = async {
        std::fs::create_dir_all(&root)?;
        std::fs::write(format!("{root}/ignore-usr1.sh"), "trap '' USR1\n")?;
        let (exec, sandbox) = client(&envd.url).map_err(|error| error.to_string())?;
        let body = request(&root, false, true).map_err(|error| error.to_string())?;
        let response = exec.handle_run_command(&sandbox, &body).await;
        let execution = started(&response).map_err(|error| error.to_string())?;
        let original = guest.last_binding()?;
        let (target, child) = wait_ready(&guest, &root)?;
        let before = guest.group_members(original)?;
        let tree_live = before.contains(&target.pid) && before.contains(&child.pid);
        require(tree_live, "target tree missing before signal")?;
        let killed = exec
            .handle_kill_command(
                &sandbox,
                "native-completion",
                &KillCommandRequest {
                    lease_id: LeaseId::new("native-completion"),
                    execution_id: execution.clone(),
                    signal,
                },
            )
            .await;
        let signaled = matches!(killed, KillCommandResponse::Signaled);
        require(signaled, "the signal must be accepted")?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let remaining = guest.group_members(original)?;
        let poll = exec
            .handle_poll_command(&sandbox, "native-completion", execution.as_str())
            .await;
        let launches = target_launches(&root)?;
        require(launches == 1, "the detached target launches once")?;
        let completed = matches!(poll, PollCommandResponse::Completed(_));
        let tree_kept = remaining.contains(&target.pid) && remaining.contains(&child.pid);
        if signal == UnixSignal::Usr1 {
            require(tree_kept, "ignored USR1 must preserve the real target tree")?;
            require(!completed, "ignored USR1 is not actual execution completion")?;
        } else {
            require(remaining.is_empty(), "fresh SIGKILL left the original group alive")?;
            require(completed, "a positively stopped native execution must complete")?;
        }
        Ok(())
    }
    .await;
    envd.shutdown().await;
    plain(guest.cleanup())?;
    plain(observed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canceled_without_end_cannot_complete_a_live_native_group() -> TestResult {
    foreground(Scenario::Canceled).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deadline_without_end_cannot_complete_a_live_native_group() -> TestResult {
    foreground(Scenario::Deadline).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_main_end_after_deadline_does_not_prove_descendant_exit() -> TestResult {
    foreground(Scenario::MainEnd).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_genuine_nonzero_native_end_preserves_completion() -> TestResult {
    foreground(Scenario::Natural).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ignored_usr1_cannot_manufacture_detached_completion() -> TestResult {
    detached_signal(UnixSignal::Usr1).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_detached_sigkill_stops_the_original_native_group() -> TestResult {
    detached_signal(UnixSignal::Kill).await
}
