// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! A Linux guest made of real local processes. The fake envd runs every process the worker starts on this machine
//! with `/proc`, process groups and sessions behaving as they do in a sandbox, so the worker's native custody
//! scripts run unchanged.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use trogon_std::env::{ReadEnv, SystemEnv};

#[path = "../command_deadline_support/mod.rs"]
pub mod command_deadline_support;
use command_deadline_support::{Reply, Request, Server};

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;

const EXEC_PREFIX: &str = "/tmp/.trogon-exec-";
const ABSENCE_DEADLINE: Duration = Duration::from_secs(10);

/// A native process group the worker reported: its leader and the leader's start time.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub pid: u32,
    pub started: u64,
}

pub struct Guest {
    /// A private directory for files the commands under test create and the test observes.
    directory: tempfile::TempDir,
    pub bindings: Mutex<Vec<Binding>>,
    pub bootstrap_calls: AtomicUsize,
    execution_directories: Mutex<Vec<String>>,
}

struct Process {
    pid: u32,
    state: char,
    group: u32,
    session: u32,
    started: u64,
}

fn processes() -> Result<Vec<Process>, Boxed> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(line) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let fields = line.rsplit_once(") ").map_or("", |(_, fields)| fields);
        let fields = fields.split_whitespace().collect::<Vec<_>>();
        // A process that exits while its stat line is read yields a partial line; it is skipped.
        if let [state, _, group, session, rest @ ..] = fields.as_slice()
            && let Some(started) = rest.get(15)
            && let (Ok(group), Ok(session), Ok(started)) = (group.parse(), session.parse(), started.parse())
        {
            found.push(Process {
                pid,
                state: state.chars().next().unwrap_or('Z'),
                group,
                session,
                started,
            });
        }
    }
    Ok(found)
}

fn kill(pid: &str) -> Result<(), Boxed> {
    let _ = Command::new("kill").args(["-KILL", "--", pid]).output()?;
    Ok(())
}

impl Guest {
    pub fn create() -> Result<std::sync::Arc<Self>, Boxed> {
        Ok(std::sync::Arc::new(Self {
            directory: tempfile::tempdir()?,
            bindings: Mutex::new(Vec::new()),
            bootstrap_calls: AtomicUsize::new(0),
            execution_directories: Mutex::new(Vec::new()),
        }))
    }

    /// A path inside the guest's private directory; commands under test write their effects there.
    pub fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    /// What a command wrote to `name` in the private directory, if it wrote anything.
    pub fn effect(&self, name: &str) -> Result<Option<Vec<u8>>, Boxed> {
        match std::fs::read(self.path(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn last_binding(&self) -> Result<Binding, Boxed> {
        self.bindings
            .lock()
            .map_err(|_| "the binding record is poisoned")?
            .last()
            .copied()
            .ok_or_else(|| "the worker established no native identity".into())
    }

    /// The live members of the process group a binding names.
    pub fn group_members(&self, binding: Binding) -> Result<Vec<u32>, Boxed> {
        let mut members = Vec::new();
        for process in processes()? {
            if process.pid == binding.pid && process.state != 'Z' && process.started != binding.started {
                return Err("native PID identity was reused".into());
            }
            if process.group == binding.pid && process.session == binding.pid && process.state != 'Z' {
                members.push(process.pid);
            }
        }
        Ok(members)
    }

    /// Kills everything this guest started and proves nothing of it remains.
    pub fn cleanup(&self) -> Result<(), Boxed> {
        let bindings = self
            .bindings
            .lock()
            .map_err(|_| "the binding record is poisoned")?
            .clone();
        let directories = self
            .execution_directories
            .lock()
            .map_err(|_| "the directory record is poisoned")?
            .clone();
        let deadline = Instant::now() + ABSENCE_DEADLINE;
        loop {
            for binding in &bindings {
                for member in self.group_members(*binding)? {
                    kill(&member.to_string())?;
                }
            }
            for process in processes()? {
                let command = std::fs::read(format!("/proc/{}/cmdline", process.pid)).unwrap_or_default();
                let command = String::from_utf8_lossy(&command);
                if process.state != 'Z' && directories.iter().any(|directory| command.contains(directory.as_str())) {
                    kill(&process.pid.to_string())?;
                }
            }
            let remaining = bindings
                .iter()
                .map(|binding| self.group_members(*binding).map(|members| members.len()))
                .sum::<Result<usize, _>>()?;
            if remaining == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err("owned native processes remain after cleanup".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        for directory in &directories {
            let _ = std::fs::remove_dir_all(directory);
        }
        Ok(())
    }

    fn run(&self, request: &Request) -> Result<Output, Boxed> {
        let frame = request.frame();
        let process = &frame["process"];
        let args = request.args();
        if args
            .iter()
            .any(|script| script.contains("runner-ready") && script.contains("nohup setsid"))
        {
            self.bootstrap_calls.fetch_add(1, Ordering::SeqCst);
        }
        let mut directories = self
            .execution_directories
            .lock()
            .map_err(|_| "the directory record is poisoned")?;
        directories.extend(
            args.iter()
                .filter(|arg| arg.starts_with(EXEC_PREFIX))
                .map(|arg| arg.to_owned()),
        );
        drop(directories);
        let mut command = Command::new(process["cmd"].as_str().ok_or("process has no command")?);
        command.args(&args).env_clear();
        if let Some(path) = SystemEnv.var_os("PATH") {
            command.env("PATH", path);
        }
        for (name, value) in process["envs"].as_object().ok_or("process has no environment")? {
            command.env(name, value.as_str().ok_or("variable is not text")?);
        }
        command.current_dir(process["cwd"].as_str().map_or_else(|| self.directory.path(), Path::new));
        Ok(command.output()?)
    }

    fn record_binding(&self, output: &Output) -> Result<(), Boxed> {
        let text = std::str::from_utf8(&output.stdout)?;
        let fields = text.split_whitespace().collect::<Vec<_>>();
        if output.status.success()
            && let [pid, started] = fields.as_slice()
            && let (Ok(pid), Ok(started)) = (pid.parse::<u32>(), started.parse::<u64>())
        {
            self.bindings
                .lock()
                .map_err(|_| "the binding record is poisoned")?
                .push(Binding { pid, started });
        }
        Ok(())
    }
}

fn start(guest: &Guest, request: &Request) -> Result<Reply, Boxed> {
    let output = guest.run(request)?;
    guest.record_binding(&output)?;
    Ok(Reply::process(
        &[output.stdout, output.stderr].concat(),
        output.status.code().unwrap_or(-1),
    ))
}

fn read_file(guest: &Guest, request: &Request) -> Result<Reply, Boxed> {
    let path = request.query("path").ok_or("no path")?;
    let _ = guest;
    Ok(match std::fs::read(path) {
        Ok(bytes) => Reply::Complete(200, bytes),
        Err(_) => Reply::json(404, json!({"message": "native file absent"})),
    })
}

/// A fake envd whose processes run in the [`Guest`].
pub struct NativeEnvd {
    pub url: String,
    server: Server,
}

impl NativeEnvd {
    pub async fn start(guest: std::sync::Arc<Guest>) -> Result<Self, Boxed> {
        let server = Server::start(move |request| {
            let result: Result<Reply, Boxed> = match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/process.Process/Start") => start(&guest, &request),
                ("GET", path) if path.starts_with("/files?") => read_file(&guest, &request),
                _ => Ok(Reply::json(404, json!({"message": "unknown native fixture operation"}))),
            };
            result.unwrap_or_else(|error| Reply::json(503, Value::from(error.to_string())))
        })
        .await?;
        Ok(Self {
            url: server.url.clone(),
            server,
        })
    }

    pub async fn shutdown(self) {
        self.server.shutdown().await;
    }
}
