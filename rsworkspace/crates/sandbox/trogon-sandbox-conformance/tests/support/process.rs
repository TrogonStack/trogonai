//! A process the fake worker runs for a lease: a real host command, or the in-process listener builtin.

use std::collections::BTreeMap;
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{Notify, watch};

/// The directory a command runs in when the request names none.
pub const DEFAULT_CWD: &str = "/tmp";

/// Argv[0] of the builtin that listens on a TCP port until killed.
pub const LISTEN: &str = "conformance-listen";

pub struct Proc {
    pub stdout: Arc<Mutex<Vec<u8>>>,
    pub stderr: Arc<Mutex<Vec<u8>>>,
    pub argv: Vec<String>,
    exit: watch::Receiver<Option<i32>>,
    kill: Arc<Notify>,
}

pub fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Proc {
    pub fn exit_code(&self) -> Option<i32> {
        *self.exit.borrow()
    }

    pub async fn finished(&self) {
        let mut exit = self.exit.clone();
        let _ = exit.wait_for(Option::is_some).await;
    }

    pub fn terminate(&self) {
        self.kill.notify_one();
    }

    pub fn output(&self) -> (Vec<u8>, Vec<u8>) {
        (locked(&self.stdout).clone(), locked(&self.stderr).clone())
    }
}

pub fn resolve_cwd(cwd: Option<&str>) -> String {
    match cwd {
        Some(cwd) if cwd.starts_with('/') => cwd.to_owned(),
        Some(cwd) => format!("{DEFAULT_CWD}/{cwd}"),
        None => DEFAULT_CWD.to_owned(),
    }
}

/// Starts `argv`. A command that cannot start is a process that already exited 127.
pub fn start(argv: &[String], cwd: Option<&str>, env: &BTreeMap<String, String>) -> Arc<Proc> {
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let kill = Arc::new(Notify::new());
    let (exit_tx, exit_rx) = watch::channel(None);
    let proc = Arc::new(Proc {
        stdout: stdout.clone(),
        stderr: stderr.clone(),
        argv: argv.to_vec(),
        exit: exit_rx,
        kill: kill.clone(),
    });
    let cwd = resolve_cwd(cwd);
    let argv = argv.to_vec();
    let env = env.clone();
    tokio::spawn(async move {
        let code = if argv.first().map(String::as_str) == Some(LISTEN) {
            listen(&argv, &kill).await
        } else {
            run(&argv, &cwd, &env, &stdout, &stderr, &kill).await
        };
        let _ = exit_tx.send(Some(code));
    });
    proc
}

async fn listen(argv: &[String], kill: &Notify) -> i32 {
    let port = argv.get(1).and_then(|port| port.parse::<u16>().ok()).unwrap_or(0);
    let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await else {
        return 1;
    };
    kill.notified().await;
    drop(listener);
    143
}

async fn run(
    argv: &[String],
    cwd: &str,
    env: &BTreeMap<String, String>,
    stdout: &Arc<Mutex<Vec<u8>>>,
    stderr: &Arc<Mutex<Vec<u8>>>,
    kill: &Notify,
) -> i32 {
    let (program, args) = argv.split_first().map_or(("", &[][..]), |(p, a)| (p.as_str(), a));
    let spawned = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("PWD", cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            locked(stderr).extend_from_slice(format!("{program}: {error}\n").as_bytes());
            return 127;
        }
    };
    let readers = [
        child
            .stdout
            .take()
            .map(|pipe| tokio::spawn(drain(pipe, stdout.clone()))),
        child
            .stderr
            .take()
            .map(|pipe| tokio::spawn(drain(pipe, stderr.clone()))),
    ];
    let pid = child.id();
    let status = tokio::select! {
        status = child.wait() => status,
        () = kill.notified() => {
            terminate_group(pid).await;
            child.wait().await
        }
    };
    for reader in readers.into_iter().flatten() {
        let _ = tokio::time::timeout(Duration::from_millis(300), reader).await;
    }
    status.map_or(1, |status| {
        status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    })
}

async fn drain(mut pipe: impl tokio::io::AsyncRead + Unpin, sink: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 4096];
    while let Ok(read) = pipe.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        locked(&sink).extend_from_slice(&chunk[..read]);
    }
}

async fn terminate_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = Command::new("kill")
            .args(["-TERM", "--", &format!("-{pid}")])
            .stderr(Stdio::null())
            .status()
            .await;
    }
}
