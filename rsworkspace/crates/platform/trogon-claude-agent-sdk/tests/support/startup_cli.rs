#![allow(
    dead_code,
    reason = "Each golden test crate that shares this scripted CLI uses a different part of it."
)]

//! A scripted Claude Code for the golden tests of `startup()` and `prewarm()`. It records every stdin line byte
//! for byte, answers initialize and the claim, and can hold its answers until it has read the lines the SDK
//! writes right after the request, which shows which writes happen before an answer and which wait for one.

use std::{
    collections::BTreeMap,
    error::Error as StdError,
    ffi::OsString,
    io,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    task::JoinHandle,
};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, JsonObject, StartupHooksRunning,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

use crate::scripted_process::{
    CapturedProcessCommand, ScriptedProcessHarness, ScriptedProcessObserver, ScriptedProcessSpawner,
};

pub type Outcome<T = ()> = Result<T, Box<dyn StdError>>;
pub type Adapters = RuntimeAdapters<FixedEnvironment, ScriptedProcessSpawner, NativeFileSystem>;

pub const WORK: &str = "/work";
pub const MARKS: &str = "CLAUDE_CODE_SDK_READS_STARTUP_MARKS";
pub const WAIT: Duration = Duration::from_secs(20);
pub const INIT: &str =
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"initialize","systemPrompt":[""]}}"#;

/// What the scripted CLI does.
#[derive(Default)]
pub struct Script {
    /// Frames written before the CLI reads anything.
    pub preface: Vec<Value>,
    /// Never answers the initialize request.
    pub no_init: bool,
    /// Answers these control subtypes with an error.
    pub fail: Vec<&'static str>,
    pub claim_error: Option<&'static str>,
    pub init: Option<Value>,
    /// Answers initialize with a success frame that has no `response`.
    pub init_omitted: bool,
    pub claim_init: Option<Value>,
    /// Answers the claim without an `initialize`.
    pub claim_init_omitted: bool,
    /// Lines read after the initialize request before any of them is answered.
    pub hold: usize,
    /// Answers the initialize request with this frame and exits at once, as a CLI that refuses to start does.
    pub error_on_init: Option<Value>,
    /// Answers the first prompt with this frame instead of a success result, then waits for the input to end.
    pub error_on_user: Option<Value>,
    /// Frames written this long after the process started, in order.
    pub timeline: Vec<(Duration, Value)>,
    /// The exit code the process reports once the input ended or it refused to start.
    pub exit_code: i32,
    /// Holds the clock after each timeline frame until the SDK noted it as a startup hook frame.
    pub hooks: Option<HookGate>,
}

/// Counts the startup hook frames the SDK noted, through `onStartupHooksRunning`, so the scripted CLI can hold a
/// paused clock until the hook frame it wrote arrived. The frame crosses the relay process, and a paused tokio clock
/// advances to the next timer whenever the runtime waits on that real IO, so without the hold the SDK could read a
/// frame written at 0 seconds only after its own 60 second deadline.
#[derive(Clone, Default)]
pub struct HookGate(Arc<(Mutex<usize>, Condvar)>);

impl HookGate {
    pub fn callback(&self) -> StartupHooksRunning {
        let gate = self.clone();
        Arc::new(move |_| gate.noted())
    }

    fn noted(&self) {
        let (count, noted) = &*self.0;
        if let Ok(mut count) = count.lock() {
            *count += 1;
        }
        noted.notify_all();
    }

    /// Waits until `frames` frames were noted. The wait runs on a blocking thread because the current-thread
    /// runtime does not advance a paused clock while a blocking task is running.
    async fn hold_until(&self, frames: usize) -> io::Result<()> {
        let gate = self.clone();
        tokio::task::spawn_blocking(move || {
            let (count, noted) = &*gate.0;
            let count = count.lock().map_err(|_| io::Error::other("hook gate poisoned"))?;
            let (count, waited) = noted
                .wait_timeout_while(count, WAIT, |count| *count < frames)
                .map_err(|_| io::Error::other("hook gate poisoned"))?;
            drop(count);
            if waited.timed_out() {
                return Err(io::Error::other("the SDK did not note the hook frame"));
            }
            Ok(())
        })
        .await
        .map_err(io::Error::other)?
    }
}

pub fn uuid(last: u8) -> String {
    format!("{:08}-{:04}-4000-8000-{:012}", 0, 0, last)
}

pub fn mark(text: &str) -> Value {
    json!({"type":"keep_alive","startup_mark":text})
}

pub fn hook_started(id: &str, event: &str) -> Value {
    json!({"type":"system","subtype":"hook_started","hook_id":id,"hook_event":event,"hook_name":"h","uuid":uuid(9),"session_id":"s"})
}

pub fn hook_response(id: &str, event: &str) -> Value {
    json!({"type":"system","subtype":"hook_response","hook_id":id,"hook_event":event,"hook_name":"h","outcome":"success","output":"","stdout":"","stderr":"","uuid":uuid(10),"session_id":"s"})
}

pub fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    }
}

pub async fn send(stdout: &mut tokio::io::DuplexStream, frame: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    stdout.write_all(&bytes).await
}

pub async fn answer(stdout: &mut tokio::io::DuplexStream, script: &Script, line: &str) -> io::Result<()> {
    let request: Value = serde_json::from_str(line)?;
    let id = request["request_id"].clone();
    let reply = |response: Value| json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}});
    let omitted = json!({"type":"control_response","response":{"subtype":"success","request_id":id}});
    let refuse =
        |error: &str| json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":error}});
    let frame = if request["type"] == "user" {
        script.error_on_user.clone().unwrap_or_else(|| json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,"num_turns":1,
            "result":"ok","session_id":"s","total_cost_usd":0,"usage":{},"modelUsage":{},"permission_denials":[],"uuid":uuid(2)}))
    } else {
        let subtype = request["request"]["subtype"].as_str().unwrap_or_default();
        if subtype == "initialize" && script.init_omitted {
            omitted
        } else if subtype == "initialize" {
            reply(
                script
                    .init
                    .clone()
                    .unwrap_or_else(|| json!({"hook_on_failure_supported":true})),
            )
        } else if subtype == "claim_session" {
            let mut answer =
                json!({"cwd":request["request"]["cwd"],"session_id":"sess-1","parked_ms":12,"sdk_mcp_settled":true});
            if let Some(answer) = answer.as_object_mut()
                && !script.claim_init_omitted
            {
                let initialize = script.claim_init.clone();
                answer.insert(
                    "initialize".to_owned(),
                    initialize.unwrap_or_else(|| json!({"commands":[],"agents":[]})),
                );
            }
            match script.claim_error {
                Some(error) => refuse(error),
                None => reply(answer),
            }
        } else if script.fail.contains(&subtype) {
            refuse(&format!("{subtype} failed"))
        } else {
            reply(json!({}))
        }
    };
    send(stdout, &frame).await
}

/// Records every line written to stdin, byte for byte, and answers as `script` says.
pub async fn serve(process: ScriptedProcessHarness, script: Script) -> io::Result<Vec<String>> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut written = Vec::new();
    for frame in &script.preface {
        send(&mut stdout, frame).await?;
    }
    let mut held: Option<Vec<String>> = None;
    let origin = tokio::time::Instant::now();
    let mut timeline: std::collections::VecDeque<(Duration, Value)> = script.timeline.iter().cloned().collect();
    let mut sent = 0;
    loop {
        let next = timeline.front().map(|(after, _)| origin + *after);
        let due = async {
            match next {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };
        let line = tokio::select! {
            () = due => {
                if let Some((_, frame)) = timeline.pop_front() {
                    send(&mut stdout, &frame).await?;
                    sent += 1;
                    if let Some(gate) = &script.hooks {
                        gate.hold_until(sent).await?;
                    }
                }
                continue;
            }
            line = lines.next_line() => line?,
        };
        let Some(line) = line else {
            break;
        };
        written.push(line.clone());
        if let Some(frame) = &script.error_on_init
            && line.contains("\"subtype\":\"initialize\"")
        {
            send(&mut stdout, frame).await?;
            break;
        }
        if let Some(batch) = held.as_mut() {
            batch.push(line);
            if batch.len() < script.hold {
                continue;
            }
            for line in held.take().unwrap_or_default() {
                answer(&mut stdout, &script, &line).await?;
            }
            continue;
        }
        if script.no_init {
            continue;
        }
        answer(&mut stdout, &script, &line).await?;
        if held.is_none() && script.hold > 0 && written.len() == 1 {
            held = Some(Vec::new());
        }
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(script.exit_code);
    Ok(written)
}

pub struct Run {
    pub server: JoinHandle<io::Result<Vec<String>>>,
    pub observer: ScriptedProcessObserver,
}

impl Run {
    pub fn command(&self) -> Outcome<CapturedProcessCommand> {
        Ok(self
            .observer
            .commands()?
            .into_iter()
            .next()
            .ok_or("no process was spawned")?)
    }

    pub fn spawned(&self) -> Outcome<usize> {
        Ok(self.observer.commands()?.len())
    }

    /// Waits for the CLI to see end of input and returns what it read, request ids normalized.
    pub async fn lines(self) -> Outcome<Vec<String>> {
        let written = tokio::time::timeout(WAIT, self.server).await???;
        written.iter().map(|line| normalize(line)).collect()
    }
}

pub fn normalize(line: &str) -> Outcome<String> {
    const KEY: &str = "\"request_id\":\"";
    let Some(start) = line.find(KEY).map(|index| index + KEY.len()) else {
        return Ok(line.to_owned());
    };
    let end = line[start..].find('"').map_or(line.len(), |index| start + index);
    let id = &line[start..end];
    if id.len() > 13
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
    {
        return Err(format!("unexpected request id {id:?}").into());
    }
    Ok(format!("{}<id>{}", &line[..start], &line[end..]))
}

pub fn start(script: Script, variables: &[(&str, &str)]) -> (Adapters, Run) {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = harness.observer.clone();
    let server = tokio::spawn(serve(harness, script));
    let environment = FixedEnvironment {
        directory: PathBuf::from(WORK),
        variables: variables
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect::<BTreeMap<_, _>>(),
    };
    (
        RuntimeAdapters {
            environment,
            process,
            filesystem: NativeFileSystem,
        },
        Run { server, observer },
    )
}

pub fn options(cwd: &str) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        cwd: Some(cwd.into()),
        ..ClaudeAgentOptions::default()
    }
}

pub fn arguments(command: &CapturedProcessCommand) -> Vec<String> {
    command
        .arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

pub const COLD_ARGUMENTS: [&str; 5] = [
    "--output-format",
    "stream-json",
    "--verbose",
    "--input-format",
    "stream-json",
];

pub fn user(text: &str) -> String {
    format!(
        r#"{{"type":"user","session_id":"","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}},"parent_tool_use_id":null}}"#
    )
}

pub fn control(body: &str) -> String {
    format!(r#"{{"request_id":"<id>","type":"control_request","request":{body}}}"#)
}

pub fn claim_line(cwd: &str) -> String {
    control(&format!(
        r#"{{"subtype":"claim_session","cwd":"{cwd}","sdk_mcp_servers":[],"include_initialize":true}}"#
    ))
}

pub fn owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}
