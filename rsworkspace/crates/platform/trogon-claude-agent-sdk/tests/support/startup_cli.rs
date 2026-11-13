#![allow(
    dead_code,
    reason = "Each golden test crate that shares this scripted CLI uses a different part of it."
)]

//! A scripted Claude Code for the golden tests of `startup()` and `prewarm()`. It records every stdin line byte
//! for byte, answers initialize and the claim, and can hold its answers until it has read the lines the SDK
//! writes right after the request, which shows which writes happen before an answer and which wait for one.

use std::{collections::BTreeMap, error::Error as StdError, ffi::OsString, io, path::PathBuf, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    task::JoinHandle,
};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, JsonObject,
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
    pub claim_init: Option<Value>,
    /// Lines read after the initialize request before any of them is answered.
    pub hold: usize,
    /// Answers a user message with this frame and exits at once instead of the success result.
    /// The exit code the process reports once the input ended or an error result was sent.
    pub exit_code: i32,
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
    let refuse =
        |error: &str| json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":error}});
    let frame = if request["type"] == "user" {
        json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,"num_turns":1,
            "result":"ok","session_id":"s","total_cost_usd":0,"usage":{},"modelUsage":{},"permission_denials":[],"uuid":uuid(2)})
    } else {
        let subtype = request["request"]["subtype"].as_str().unwrap_or_default();
        if subtype == "initialize" {
            reply(
                script
                    .init
                    .clone()
                    .unwrap_or_else(|| json!({"hook_on_failure_supported":true})),
            )
        } else if subtype == "claim_session" {
            match script.claim_error {
                Some(error) => refuse(error),
                None => reply(
                    json!({"cwd":request["request"]["cwd"],"session_id":"sess-1","parked_ms":12,"sdk_mcp_settled":true,
                    "initialize":script.claim_init.clone().unwrap_or_else(|| json!({"commands":[],"agents":[]}))}),
                ),
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
    while let Some(line) = lines.next_line().await? {
        written.push(line.clone());
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
