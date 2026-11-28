//! Sorts what a probe command printed into closed labels, so a diagnostic receipt never carries raw guest output.

use serde::Serialize;
use trogon_sandbox_contract::{CommandOutput, RunCommandResponse};

/// The largest stdout a probe may print and still count as usable.
pub const OUTPUT_BOUND: usize = 4096;

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    Zero,
    One,
    UsageTwo,
    NotExecutable,
    NotFound,
    Signaled,
    Other,
    Unavailable,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Stderr {
    Empty,
    MissingTool,
    UnsupportedOption,
    SyntaxError,
    PermissionDenied,
    NetworkError,
    PythonImportError,
    OtherRedacted,
    Unavailable,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Errno {
    Eperm,
    Eintr,
    Eacces,
    Epipe,
    Enetunreach,
    Econnaborted,
    Econnreset,
    Etimedout,
    Econnrefused,
    Ehostunreach,
    Ecanceled,
    Other,
    Ambiguous,
    Malformed,
    NotReported,
    Unavailable,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Stdout {
    Nc,
    Python3,
    NoProbe,
    Connected,
    Denied,
    Empty,
    Unknown,
    Unavailable,
}

/// How the worker answered a probe command.
#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Completed,
    Denied,
    Unexpected,
}

#[derive(Clone, Copy, Serialize, Debug)]
pub struct Summary {
    pub exit: Exit,
    pub timed_out: bool,
    pub truncated: bool,
    pub stderr: Stderr,
    pub errno: Errno,
    pub stdout: Stdout,
    pub within_output_bound: bool,
}

impl Summary {
    const UNAVAILABLE: Self = Self {
        exit: Exit::Unavailable,
        timed_out: false,
        truncated: false,
        stderr: Stderr::Unavailable,
        errno: Errno::Unavailable,
        stdout: Stdout::Unavailable,
        within_output_bound: false,
    };
}

#[derive(Clone, Copy, Serialize, Debug)]
pub struct Observation {
    pub phase: &'static str,
    pub role: &'static str,
    pub reply: Reply,
    pub output: Summary,
}

impl Observation {
    /// The probe completed on its own, cleanly and within bounds, so its stdout can be believed.
    pub fn usable(&self) -> bool {
        self.reply == Reply::Completed
            && self.output.exit == Exit::Zero
            && !self.output.timed_out
            && !self.output.truncated
            && self.output.stderr == Stderr::Empty
            && self.output.within_output_bound
    }
}

pub fn exit_class(value: i32) -> Exit {
    match value {
        0 => Exit::Zero,
        1 => Exit::One,
        2 => Exit::UsageTwo,
        126 => Exit::NotExecutable,
        127 => Exit::NotFound,
        128..=192 => Exit::Signaled,
        _ => Exit::Other,
    }
}

pub fn stderr_class(value: &[u8]) -> Stderr {
    if value.is_empty() {
        return Stderr::Empty;
    }
    let text = String::from_utf8_lossy(value).to_ascii_lowercase();
    let has = |markers: &[&str]| markers.iter().any(|marker| text.contains(marker));
    if has(&["modulenotfounderror", "importerror"]) {
        Stderr::PythonImportError
    } else if has(&["invalid option", "unknown option", "unrecognized option", "usage:"]) {
        Stderr::UnsupportedOption
    } else if has(&["not found", "no such file"]) {
        Stderr::MissingTool
    } else if has(&["permission denied", "operation not permitted"]) {
        Stderr::PermissionDenied
    } else if has(&["syntaxerror", "syntax error"]) {
        Stderr::SyntaxError
    } else if has(&["unreachable", "refused", "timed out", "timeout", "reset"]) {
        Stderr::NetworkError
    } else {
        Stderr::OtherRedacted
    }
}

/// The socket error a Python traceback reports, as a label. These are the guest's Linux values, whatever the host.
pub fn errno_class(value: &[u8]) -> Errno {
    let text = String::from_utf8_lossy(value).to_ascii_lowercase();
    let mut found: Option<&str> = None;
    for (index, _) in text.match_indices("[errno ") {
        let Some((code, _)) = text[index + 7..].split_once(']') else {
            return Errno::Malformed;
        };
        if code.is_empty() || code.len() > 10 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
            return Errno::Malformed;
        }
        if found.is_some_and(|previous| previous != code) {
            return Errno::Ambiguous;
        }
        found = Some(code);
    }
    match found.and_then(|code| code.parse::<u32>().ok()) {
        Some(1) => Errno::Eperm,
        Some(4) => Errno::Eintr,
        Some(13) => Errno::Eacces,
        Some(32) => Errno::Epipe,
        Some(101) => Errno::Enetunreach,
        Some(103) => Errno::Econnaborted,
        Some(104) => Errno::Econnreset,
        Some(110) => Errno::Etimedout,
        Some(111) => Errno::Econnrefused,
        Some(113) => Errno::Ehostunreach,
        Some(125) => Errno::Ecanceled,
        Some(_) => Errno::Other,
        None if found.is_some() => Errno::Malformed,
        None => Errno::NotReported,
    }
}

pub fn stdout_class(value: &[u8]) -> Stdout {
    match value.strip_suffix(b"\n").unwrap_or(value) {
        b"NC" => Stdout::Nc,
        b"PYTHON3" => Stdout::Python3,
        b"NO_PROBE" => Stdout::NoProbe,
        b"CONNECTED" => Stdout::Connected,
        b"DENIED" => Stdout::Denied,
        b"" => Stdout::Empty,
        _ => Stdout::Unknown,
    }
}

fn summarize(output: &CommandOutput) -> Summary {
    Summary {
        exit: exit_class(output.exit_code),
        timed_out: output.timed_out.is_timed_out(),
        truncated: output.truncated.is_truncated(),
        stderr: stderr_class(&output.stderr),
        errno: errno_class(&output.stderr),
        stdout: stdout_class(&output.stdout),
        within_output_bound: output.stdout.len() <= OUTPUT_BOUND,
    }
}

pub fn classify(phase: &'static str, role: &'static str, response: &RunCommandResponse) -> Observation {
    let (reply, output) = match response {
        RunCommandResponse::Completed(output) => (Reply::Completed, summarize(output)),
        RunCommandResponse::Denied(_) => (Reply::Denied, Summary::UNAVAILABLE),
        _ => (Reply::Unexpected, Summary::UNAVAILABLE),
    };
    Observation {
        phase,
        role,
        reply,
        output,
    }
}
