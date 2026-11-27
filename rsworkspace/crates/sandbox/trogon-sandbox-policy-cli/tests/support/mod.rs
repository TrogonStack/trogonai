#![allow(dead_code, reason = "each test binary uses a different part of the support code")]

use std::io::Read;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub const EXIT_TIMEOUT: Duration = Duration::from_secs(60);
pub const NO_HOME: &str = "/tmp/trogon-sandbox-policy-test-nonexistent";

/// A package binary as an absolute path, so a test that moves the command to another directory still finds it.
pub fn binary(path: &str) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// The command line under test with a home directory that holds no settings.
pub fn policy_cli() -> Command {
    let mut command = Command::new(binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy")));
    command.env("HOME", NO_HOME).env_remove("TROGON_SANDBOX_POLICY_DEBUG");
    command
}

pub struct Finished {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

impl Finished {
    pub fn code(&self) -> Option<i32> {
        self.status.code()
    }
}

/// The path as text, for passing to a command line.
pub fn utf8(path: &Path) -> Result<&str, Box<dyn std::error::Error>> {
    path.to_str().ok_or_else(|| "the path is not UTF-8".into())
}

pub fn run(args: &[&str]) -> Result<Finished, Box<dyn std::error::Error>> {
    let mut command = policy_cli();
    command.args(args);
    finish(command)
}

pub fn finish(mut command: Command) -> Result<Finished, Box<dyn std::error::Error>> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    collect(child)
}

pub fn collect(mut child: Child) -> Result<Finished, Box<dyn std::error::Error>> {
    let stdout = reader(child.stdout.take());
    let stderr = reader(child.stderr.take());
    let status = wait(&mut child)?;
    Ok(Finished {
        stdout: stdout.recv().unwrap_or_default(),
        stderr: stderr.recv().unwrap_or_default(),
        status,
    })
}

fn reader<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = pipe {
            let mut bytes = Vec::new();
            // A pipe that breaks early leaves whatever was read so far.
            let _ = pipe.read_to_end(&mut bytes);
            text = String::from_utf8_lossy(&bytes).into_owned();
        }
        // The receiver is gone only when the test already failed.
        let _ = sender.send(text);
    });
    receiver
}

pub fn wait(child: &mut Child) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() > EXIT_TIMEOUT {
            child.kill()?;
            return Err(format!("the command did not exit within {EXIT_TIMEOUT:?}").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn script(dir: &Path, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = dir.join("test.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    Ok(path.display().to_string())
}

pub fn hand_over(command: &mut Command, fd: RawFd, as_fd: RawFd) {
    // SAFETY: the closure only calls async-signal-safe `fcntl` and `dup2`.
    unsafe {
        command.pre_exec(move || {
            if fd == as_fd {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            } else if libc::dup2(fd, as_fd) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

pub struct Pipe {
    pub read: RawFd,
    pub write: RawFd,
}

impl Pipe {
    pub fn open() -> std::io::Result<Self> {
        let mut fds = [0; 2];
        // SAFETY: `fds` has room for the two descriptors `pipe` writes.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        for fd in fds {
            // SAFETY: both descriptors were just opened and belong to this value.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        Ok(Self {
            read: fds[0],
            write: fds[1],
        })
    }

    pub fn send(&self, text: &str) -> std::io::Result<()> {
        let bytes = text.as_bytes();
        // SAFETY: `bytes` is valid for its length for the whole call.
        let written = unsafe { libc::write(self.write, bytes.as_ptr().cast(), bytes.len()) };
        if usize::try_from(written).ok() == Some(bytes.len()) {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    pub fn close_write(&mut self) {
        // SAFETY: the descriptor belongs to this value and is forgotten right after.
        unsafe { libc::close(self.write) };
        self.write = -1;
    }

    pub fn close_read(&mut self) {
        // SAFETY: the descriptor belongs to this value and is forgotten right after.
        unsafe { libc::close(self.read) };
        self.read = -1;
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        for fd in [self.read, self.write] {
            if fd >= 0 {
                // SAFETY: the descriptor belongs to this value and is closed once.
                unsafe { libc::close(fd) };
            }
        }
    }
}

pub struct Watched {
    pub child: Child,
    lines: mpsc::Receiver<String>,
    seen: String,
}

impl Watched {
    pub fn spawn(mut command: Command) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stderr = child.stderr.take().ok_or("the command has no stderr pipe")?;
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut chunk) {
                if read == 0
                    || sender
                        .send(String::from_utf8_lossy(&chunk[..read]).into_owned())
                        .is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            lines,
            seen: String::new(),
        })
    }

    pub fn wait_for_stderr(&mut self, needle: &str) -> TestResult {
        let started = Instant::now();
        while !self.seen.contains(needle) {
            let left = EXIT_TIMEOUT.saturating_sub(started.elapsed());
            match self.lines.recv_timeout(left) {
                Ok(chunk) => self.seen.push_str(&chunk),
                Err(_) => {
                    return Err(format!("the command did not print {needle:?}; stderr so far: {}", self.seen).into());
                }
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Finished, Box<dyn std::error::Error>> {
        let stdout = reader(self.child.stdout.take());
        let status = wait(&mut self.child)?;
        while let Ok(chunk) = self.lines.recv_timeout(Duration::from_secs(2)) {
            self.seen.push_str(&chunk);
        }
        Ok(Finished {
            stdout: stdout.recv().unwrap_or_default(),
            stderr: self.seen,
            status,
        })
    }
}
