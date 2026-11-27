#![cfg(unix)]

mod command_support;
mod support;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use command_support::native::Directory;
use command_support::{
    Boxed, CMD, Reply, Request, Server, TestResult, body_of, completed, error_reply, exit_record, is_control, locked,
    polled, run_request, sandbox, started, terminal,
};
use serde_json::{Value, json};
use trogon_sandbox_contract::{CommandOutput, LeaseId, ReadFileRequest, ReadFileResponse, RunCommandRequest};
use trogon_sandbox_vercel_worker::{LeaseSandbox, VercelExec};

const LEASE: &str = "binary-lease";
const SESSION: &str = "/v2/sandboxes/sessions/session";

#[derive(Clone)]
struct NativeOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit: i32,
}

struct Native {
    directory: Directory,
    outputs: Mutex<HashMap<String, NativeOutput>>,
    user_starts: AtomicUsize,
}

fn execute(directory: &Directory, body: &Value, id: &str) -> Result<NativeOutput, Boxed> {
    let (mut command, stdout, stderr) = directory.command(body, id)?;
    let status = command.spawn()?.wait()?;
    Ok(NativeOutput {
        stdout: std::fs::read(stdout)?,
        stderr: std::fs::read(stderr)?,
        exit: status.code().ok_or("the native command did not exit normally")?,
    })
}

impl Native {
    fn respond(&self, request: &Request) -> Result<Reply, Boxed> {
        let route = request.route();
        if request.method == "POST" && route == CMD {
            let body = body_of(request);
            if !is_control(&body) {
                self.user_starts.fetch_add(1, Ordering::SeqCst);
            }
            let id = format!("command-{}", locked(&self.outputs).len() + 1);
            let output = execute(&self.directory, &body, &id)?;
            let record = exit_record(&id, output.exit);
            locked(&self.outputs).insert(id, output);
            return Ok(terminal(&body, &record));
        }
        if request.method == "POST" && route == format!("{SESSION}/fs/read") {
            let path = Path::new(body_of(request)["path"].as_str().ok_or("no path")?).canonicalize();
            return Ok(match path {
                Ok(path) if path.starts_with(&self.directory.path) => Reply::Complete(200, std::fs::read(path)?),
                _ => error_reply(404, "fixture file missing"),
            });
        }
        let suffix = route.strip_prefix(&format!("{SESSION}/cmd/")).unwrap_or_default();
        let (id, logs) = suffix.strip_suffix("/logs").map_or((suffix, false), |id| (id, true));
        let Some(output) = locked(&self.outputs).get(id).cloned() else {
            return Ok(error_reply(404, "unknown native fixture request"));
        };
        if !logs {
            return Ok(Reply::json(200, exit_record(id, output.exit)));
        }
        // Vercel documents log data as strings, not a binary encoding.
        let stdout = json!({"stream": "stdout", "data": String::from_utf8_lossy(&output.stdout)});
        let stderr = json!({"stream": "stderr", "data": String::from_utf8_lossy(&output.stderr)});
        Ok(Reply::Complete(200, format!("{stdout}\n{stderr}\n").into_bytes()))
    }
}

struct Fixture {
    server: Server,
    native: Arc<Native>,
    exec: VercelExec,
    sandbox: LeaseSandbox,
}

impl Fixture {
    async fn new() -> Result<Self, Boxed> {
        let directory = Directory::new("vercel-binary-", &["base64"])?;
        let native = Arc::new(Native {
            directory,
            outputs: Mutex::new(HashMap::new()),
            user_starts: AtomicUsize::new(0),
        });
        let state = native.clone();
        let server = Server::start(move |request| command_support::fixture_reply(state.respond(&request))).await?;
        let exec = command_support::exec_within(&server, 15)?;
        Ok(Self {
            server,
            native,
            exec,
            sandbox: sandbox(),
        })
    }

    fn root(&self) -> &Path {
        &self.native.directory.path
    }

    fn starts(&self) -> usize {
        self.native.user_starts.load(Ordering::SeqCst)
    }

    /// Runs `request` and, for a detached one, polls it to completion.
    async fn finish(&self, request: &RunCommandRequest, detached: bool) -> Result<CommandOutput, Boxed> {
        let response = self.exec.handle_run_command(&self.sandbox, request).await;
        if !detached {
            return completed(response);
        }
        let id = started(response)?;
        polled(self.exec.handle_poll_command(&self.sandbox, LEASE, id.as_str()).await)
    }

    async fn run(&self, argv: &[&str], detached: bool, cap: Option<u64>) -> Result<CommandOutput, Boxed> {
        let extra = json!({"cwd": self.root(), "detached": detached, "max_output_bytes": cap});
        let request = run_request(LEASE, json!(argv), extra)?;
        self.finish(&request, detached).await
    }
}

async fn binary_case(detached: bool) -> TestResult {
    let fixture = Fixture::new().await?;
    let stdout = b"line one\n\xff\xfe binary tail\0";
    let stderr = b"error\0\x80\xff tail\n";
    let file = fixture.root().join("source.bin");
    std::fs::write(&file, stdout)?;
    let read = fixture
        .exec
        .handle_read_file(
            &fixture.sandbox,
            &ReadFileRequest {
                lease_id: LeaseId::new(LEASE),
                path: file.to_string_lossy().into_owned(),
                as_user: None,
            },
        )
        .await;
    assert_eq!(read, ReadFileResponse::Content { data: stdout.to_vec() });
    let missing = ReadFileRequest {
        lease_id: LeaseId::new(LEASE),
        path: fixture.root().join("absent.bin").to_string_lossy().into_owned(),
        as_user: None,
    };
    assert_eq!(
        fixture.exec.handle_read_file(&fixture.sandbox, &missing).await,
        ReadFileResponse::NotFound
    );
    let command = "cat \"$1\"; printf 'error\\000\\200\\377 tail\\n' >&2; exit 7";
    let body = json!({"command": "sh", "args": ["-c", command, "binary-fixture", file], "cwd": fixture.root()});
    let raw = execute(&fixture.native.directory, &body, "independent-native")?;
    assert_eq!(
        (raw.stdout.as_slice(), raw.stderr.as_slice(), raw.exit),
        (&stdout[..], &stderr[..], 7)
    );
    let file = file.to_str().ok_or("the fixture path is not text")?;
    let output = fixture
        .run(&["sh", "-c", command, "binary-fixture", file], detached, None)
        .await?;
    assert!(
        fixture.starts() > 0,
        "the call executed a real native command through HTTP"
    );
    assert_eq!(output.exit_code, 7);
    command_support::assert_clean(&output);
    assert_eq!(output.stdout, stdout, "bytes survive Vercel's text-only logs");
    assert_eq!(output.stderr, stderr);
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_binary_stdout_and_stderr_survive_the_text_log_api() -> TestResult {
    binary_case(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_binary_stdout_and_stderr_survive_the_text_log_api() -> TestResult {
    binary_case(true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_utf8_streams_and_nonzero_exit_remain_exact() -> TestResult {
    for detached in [false, true] {
        let fixture = Fixture::new().await?;
        let script = "printf 'héllo\\n'; printf 'érr\\n' >&2; exit 7";
        let output = fixture.run(&["sh", "-c", script], detached, None).await?;
        assert_eq!(output.stdout, "héllo\n".as_bytes());
        assert_eq!(output.stderr, "érr\n".as_bytes());
        assert_eq!(output.exit_code, 7);
        command_support::assert_clean(&output);
        fixture.server.shutdown().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_exit_124_is_not_an_inferred_deadline() -> TestResult {
    let fixture = Fixture::new().await?;
    let output = fixture
        .run(&["sh", "-c", "printf 'own exit'; exit 124"], false, None)
        .await?;
    assert_eq!(output.exit_code, 124);
    assert_eq!(output.stdout, b"own exit");
    command_support::assert_clean(&output);
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_requested_per_stream_cap_still_preserves_native_head_and_tail() -> TestResult {
    let fixture = Fixture::new().await?;
    let stdout = format!("HEAD{}TAIL", "a".repeat(56));
    let stderr = format!("ERRH{}ERRT", "b".repeat(56));
    let script = "printf '%s' \"$1\"; printf '%s' \"$2\" >&2; exit 7";
    let output = fixture
        .run(&["sh", "-c", script, "cap-fixture", &stdout, &stderr], false, Some(8))
        .await?;
    assert_eq!(output.stdout, b"HEAD\n[... 56 bytes truncated ...]\nTAIL");
    assert_eq!(output.stderr, b"ERRH\n[... 56 bytes truncated ...]\nERRT");
    assert_eq!(output.exit_code, 7);
    assert!(output.truncated.is_truncated() && !output.timed_out.is_timed_out());
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_execution_preserves_effective_argv_cwd_and_environment() -> TestResult {
    for detached in [false, true] {
        let fixture = Fixture::new().await?;
        let cwd = fixture.root().join("work space");
        std::fs::create_dir(&cwd)?;
        let environment = "a value with 'quotes' = $literal\nand a newline";
        let first = "argument with spaces";
        let second = "'quoted' $literal\nsecond line";
        let script = "printf '%s\\n' \"$PWD\" \"$CAPTURE_CASE_VALUE\" \"$1\" \"$2\"; printf '%s' \"$3\" >&2; exit 9";
        let command = json!(["sh", "-c", script, "argv-fixture", first, second, "stderr argument"]);
        let extra = json!({"cwd": cwd, "env": {"CAPTURE_CASE_VALUE": environment}, "detached": detached});
        let output = fixture.finish(&run_request(LEASE, command, extra)?, detached).await?;
        assert_eq!(fixture.starts(), 1);
        assert_eq!(
            output.stdout,
            format!("{}\n{environment}\n{first}\n{second}\n", cwd.display()).as_bytes()
        );
        assert_eq!(output.stderr, b"stderr argument");
        assert_eq!(output.exit_code, 9);
        command_support::assert_clean(&output);
        fixture.server.shutdown().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lease_bound_native_command_outlives_timeout_seconds_without_a_finite_deadline() -> TestResult {
    let fixture = Fixture::new().await?;
    let script = "sleep 2; printf 'lease\\000\\377 tail'; printf 'error\\376 tail' >&2; exit 9";
    let extra = json!({"cwd": fixture.root(), "timeout_seconds": 1, "detached": true, "deadline": "lease"});
    let request = run_request(LEASE, json!(["sh", "-c", script]), extra)?;
    let started_at = Instant::now();
    let output = fixture.finish(&request, true).await?;
    assert_eq!(fixture.starts(), 1);
    assert!(started_at.elapsed() >= Duration::from_secs(2));
    assert_eq!(output.stdout, b"lease\0\xff tail");
    assert_eq!(output.stderr, b"error\xfe tail");
    assert_eq!(output.exit_code, 9);
    command_support::assert_clean(&output);
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_outside_the_fixture_is_not_served() -> TestResult {
    let fixture = Fixture::new().await?;
    let outside = ReadFileRequest {
        lease_id: LeaseId::new(LEASE),
        path: "/".into(),
        as_user: None,
    };
    assert_eq!(
        fixture.exec.handle_read_file(&fixture.sandbox, &outside).await,
        ReadFileResponse::NotFound
    );
    fixture.server.shutdown().await;
    Ok(())
}
