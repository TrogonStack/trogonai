// Shared by the command test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

#[cfg(unix)]
pub mod native;

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, KillCommandRequest, LeaseId, PollCommandResponse, RunCommandRequest,
    RunCommandResponse, UnixSignal,
};
use trogon_sandbox_vercel_worker::{LeaseSandbox, VercelApi, VercelExec};

pub use crate::support::{Boxed, Reply, Request, Server, TestResult};

pub const CMD: &str = "/v2/sandboxes/sessions/session/cmd";
pub const CONTROL_MARKER: &str = "sandbox-capture-control";

pub fn sandbox() -> LeaseSandbox {
    LeaseSandbox {
        name: "sandbox".into(),
        session_id: "session".into(),
        routes: BTreeMap::new(),
    }
}

/// An exec whose transport timeout is `seconds`, for tests that must outlive the default.
pub fn exec_within(server: &Server, seconds: u64) -> Result<VercelExec, Boxed> {
    Ok(VercelExec::new(VercelApi::new(
        server.config()?,
        Duration::from_secs(seconds),
    )?))
}

pub fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A request in the shape clients send it; `extra` is merged over the defaults.
pub fn run_request(lease: &str, command: Value, extra: Value) -> Result<RunCommandRequest, serde_json::Error> {
    let mut value = json!({"lease_id": lease, "command": command, "timeout_seconds": 5});
    if let (Some(base), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    serde_json::from_value(value)
}

pub fn kill_request(lease: &str, execution: &str, signal: UnixSignal) -> KillCommandRequest {
    KillCommandRequest {
        lease_id: LeaseId::new(lease),
        execution_id: ExecutionId::new(execution),
        signal,
    }
}

/// Sets the environment variable of a request through the typed `CommandEnv`.
pub fn with_env(
    mut request: RunCommandRequest,
    name: &str,
    value: &str,
) -> Result<RunCommandRequest, trogon_sandbox_contract::CommandEnvError> {
    let mut vars = request.env.as_map().clone();
    vars.insert(name.to_owned(), value.to_owned());
    request.env = trogon_sandbox_contract::CommandEnv::new(vars)?;
    Ok(request)
}

pub fn body_of(request: &Request) -> Value {
    request.json().unwrap_or_default()
}

pub fn is_control(body: &Value) -> bool {
    body.pointer("/args/2").and_then(Value::as_str) == Some(CONTROL_MARKER)
}

/// A capture control that reads the retained capture carries the status as its fifth argument.
pub fn is_read(body: &Value) -> bool {
    body.pointer("/args/4").is_some()
}

pub fn ndjson(value: &Value) -> Reply {
    Reply::Complete(200, format!("{value}\n").into_bytes())
}

pub fn error_reply(status: u16, message: &str) -> Reply {
    Reply::json(status, json!({"error": {"message": message}}))
}

/// A launch answers as a stream when it waits and as one JSON document otherwise.
pub fn terminal(body: &Value, record: &Value) -> Reply {
    if body["wait"] == true {
        ndjson(record)
    } else {
        Reply::json(200, record.clone())
    }
}

pub fn log_reply(data: &str) -> Reply {
    ndjson(&json!({"stream": "stdout", "data": data}))
}

pub fn exit_record(id: &str, exit: impl Into<Value>) -> Value {
    json!({"command": {"id": id, "exitCode": exit.into()}})
}

pub fn fixture_reply(result: Result<Reply, Boxed>) -> Reply {
    result.unwrap_or_else(|error| error_reply(500, &error.to_string()))
}

pub fn started(response: RunCommandResponse) -> Result<ExecutionId, Boxed> {
    let described = format!("{response:?}");
    Ok(match response {
        RunCommandResponse::Started(started) => started.execution_id,
        _ => return Err(described.into()),
    })
}

pub fn completed(response: RunCommandResponse) -> Result<CommandOutput, Boxed> {
    let described = format!("{response:?}");
    Ok(match response {
        RunCommandResponse::Completed(output) => output,
        _ => return Err(described.into()),
    })
}

pub fn polled(response: PollCommandResponse) -> Result<CommandOutput, Boxed> {
    let described = format!("{response:?}");
    Ok(match response {
        PollCommandResponse::Completed(output) => output,
        _ => return Err(described.into()),
    })
}

/// A command ran to its own end: neither the deadline nor the output cap touched it.
pub fn assert_clean(output: &CommandOutput) {
    assert!(!output.timed_out.is_timed_out() && output.truncated.is_complete());
}
