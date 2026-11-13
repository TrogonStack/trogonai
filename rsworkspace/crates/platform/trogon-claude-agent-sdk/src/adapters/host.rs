use crate::constants::MAX_BUFFER_ERRNO;

use std::{
    ffi::OsStr,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

/// Reads a whole file exactly as [`std::fs::read`] does.
pub trait ReadFile {
    type Error;

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, Self::Error>;
}

/// Opens a file for reading and drops the handle, the way `access(path, R_OK)` answers readability.
pub trait CheckReadable {
    type Error;

    fn check_readable(&self, path: &Path) -> Result<(), Self::Error>;
}

/// Kind of a directory entry, as `readdir` with file types reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// One entry of a directory listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    pub name: String,
    pub kind: DirectoryEntryKind,
}

/// Lists a directory exactly as [`std::fs::read_dir`] does, with each entry's own file type.
pub trait ReadDirectory {
    type Error;

    fn read_directory(&self, path: &Path) -> Result<Vec<DirectoryEntry>, Self::Error>;
}

/// What `lstat` reports about one path: its own kind (a symbolic link is not followed) and its size in bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkMetadata {
    pub kind: DirectoryEntryKind,
    pub len: u64,
}

/// Reads metadata exactly as [`std::fs::symlink_metadata`] does.
pub trait ReadSymlinkMetadata {
    type Error;

    fn symlink_metadata(&self, path: &Path) -> Result<LinkMetadata, Self::Error>;
}

/// Reads a symbolic link's text exactly as [`std::fs::read_link`] does.
pub trait ReadLink {
    type Error;

    fn read_link(&self, path: &Path) -> Result<PathBuf, Self::Error>;
}

/// The operating system the process runs on, as `process.platform` names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperatingSystem {
    Darwin,
    Win32,
    Linux,
    Other(String),
}

/// Reports the platform and the account name the process runs as.
pub trait HostIdentity {
    fn operating_system(&self) -> OperatingSystem;

    /// The login name of the current user, as `os.userInfo().username` returns it.
    fn username(&self) -> Option<String>;
}

/// One `execFile` request.
#[derive(Clone, Debug)]
pub struct ExecuteFileRequest<'a> {
    pub program: &'a OsStr,
    pub args: &'a [&'a OsStr],
    pub timeout: Duration,
    /// Largest stdout or stderr, in bytes, before the child is killed.
    pub max_buffer: Option<usize>,
}

/// How a spawned file ended, with the same classification `child_process.execFile` callbacks give.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecuteFileStatus {
    Ok,
    Exited,
    Timeout,
    Killed,
    SpawnError,
}

/// Everything one `execFile` call reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecuteFileOutcome {
    pub stdout: String,
    pub stderr: String,
    pub status: ExecuteFileStatus,
    pub exit_code: Option<i32>,
    pub errno: Option<String>,
    pub signal: Option<String>,
    pub duration_ms: u128,
}

/// Runs a program to completion the way `child_process.execFile` does.
pub trait ExecuteFile {
    fn execute_file(&self, request: &ExecuteFileRequest<'_>) -> ExecuteFileOutcome;
}

/// The native host: the real file system, directory listing, process and platform.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeHost;

impl ReadFile for NativeHost {
    type Error = io::Error;

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, Self::Error> {
        std::fs::read(path)
    }
}

impl CheckReadable for NativeHost {
    type Error = io::Error;

    fn check_readable(&self, path: &Path) -> Result<(), Self::Error> {
        std::fs::File::open(path).map(|_| ())
    }
}

impl ReadDirectory for NativeHost {
    type Error = io::Error;

    fn read_directory(&self, path: &Path) -> Result<Vec<DirectoryEntry>, Self::Error> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let kind = if file_type.is_symlink() {
                DirectoryEntryKind::Symlink
            } else if file_type.is_file() {
                DirectoryEntryKind::File
            } else if file_type.is_dir() {
                DirectoryEntryKind::Directory
            } else {
                DirectoryEntryKind::Other
            };
            entries.push(DirectoryEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
            });
        }
        Ok(entries)
    }
}

impl ReadSymlinkMetadata for NativeHost {
    type Error = io::Error;

    fn symlink_metadata(&self, path: &Path) -> Result<LinkMetadata, Self::Error> {
        let metadata = std::fs::symlink_metadata(path)?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            DirectoryEntryKind::Symlink
        } else if file_type.is_file() {
            DirectoryEntryKind::File
        } else if file_type.is_dir() {
            DirectoryEntryKind::Directory
        } else {
            DirectoryEntryKind::Other
        };
        Ok(LinkMetadata {
            kind,
            len: metadata.len(),
        })
    }
}

impl ReadLink for NativeHost {
    type Error = io::Error;

    fn read_link(&self, path: &Path) -> Result<PathBuf, Self::Error> {
        std::fs::read_link(path)
    }
}

impl HostIdentity for NativeHost {
    fn operating_system(&self) -> OperatingSystem {
        if cfg!(target_os = "macos") {
            OperatingSystem::Darwin
        } else if cfg!(windows) {
            OperatingSystem::Win32
        } else if cfg!(target_os = "linux") {
            OperatingSystem::Linux
        } else {
            OperatingSystem::Other(std::env::consts::OS.to_owned())
        }
    }

    fn username(&self) -> Option<String> {
        ["USER", "LOGNAME", "USERNAME"].iter().find_map(|name| {
            super::EnvironmentVariable::var_os(&super::SystemEnvironment, OsStr::new(name))
                .map(|value| value.to_string_lossy().into_owned())
                .filter(|value| !value.is_empty())
        })
    }
}

impl super::EnvironmentVariable for NativeHost {
    fn var_os(&self, key: &OsStr) -> Option<std::ffi::OsString> {
        super::EnvironmentVariable::var_os(&super::SystemEnvironment, key)
    }

    fn vars_os(&self) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        super::EnvironmentVariable::vars_os(&super::SystemEnvironment)
    }
}

impl super::WallClock for NativeHost {
    type Time = SystemTime;

    fn now(&self) -> SystemTime {
        super::WallClock::now(&super::SystemClock)
    }
}

impl super::CurrentDirectory for NativeHost {
    type Error = io::Error;

    fn current_dir(&self) -> Result<PathBuf, Self::Error> {
        super::CurrentDirectory::current_dir(&super::SystemEnvironment)
    }
}

fn errno_name(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::PermissionDenied => "EACCES",
        io::ErrorKind::WouldBlock => "EAGAIN",
        io::ErrorKind::OutOfMemory => "ENOMEM",
        io::ErrorKind::NotADirectory => "ENOTDIR",
        _ => "UNKNOWN",
    }
    .to_owned()
}

fn drain(
    mut stream: impl Read + Send + 'static,
    limit: Option<usize>,
    over: Arc<AtomicBool>,
) -> thread::JoinHandle<(Vec<u8>, bool)> {
    thread::spawn(move || {
        let mut collected = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return (collected, false),
                Ok(read) => {
                    collected.extend_from_slice(&chunk[..read]);
                    if limit.is_some_and(|limit| collected.len() > limit) {
                        over.store(true, Ordering::SeqCst);
                        return (collected, true);
                    }
                }
            }
        }
    })
}

impl ExecuteFile for NativeHost {
    fn execute_file(&self, request: &ExecuteFileRequest<'_>) -> ExecuteFileOutcome {
        let started = Instant::now();
        let finish = |mut outcome: ExecuteFileOutcome| {
            outcome.duration_ms = started.elapsed().as_millis();
            outcome
        };
        let mut command = Command::new(request.program);
        command
            .args(request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return finish(ExecuteFileOutcome {
                    stdout: String::new(),
                    stderr: String::new(),
                    status: ExecuteFileStatus::SpawnError,
                    exit_code: None,
                    errno: Some(errno_name(&error)),
                    signal: None,
                    duration_ms: 0,
                });
            }
        };
        let over = Arc::new(AtomicBool::new(false));
        let stdout = child
            .stdout
            .take()
            .map(|stream| drain(stream, request.max_buffer, over.clone()));
        let stderr = child
            .stderr
            .take()
            .map(|stream| drain(stream, request.max_buffer, over.clone()));
        let mut timed_out = false;
        let mut overflow = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if over.load(Ordering::SeqCst) || started.elapsed() >= request.timeout {
                timed_out = started.elapsed() >= request.timeout;
                let _ = child.kill();
                break child.wait().ok();
            }
            thread::sleep(Duration::from_millis(5));
        };
        let mut collect = |handle: Option<thread::JoinHandle<(Vec<u8>, bool)>>| {
            let (bytes, over) = handle.and_then(|handle| handle.join().ok()).unwrap_or_default();
            overflow |= over;
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let stdout = collect(stdout);
        let stderr = collect(stderr);
        let make = |status, exit_code, errno, signal| {
            finish(ExecuteFileOutcome {
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                status,
                exit_code,
                errno,
                signal,
                duration_ms: 0,
            })
        };
        if overflow {
            return make(
                ExecuteFileStatus::SpawnError,
                None,
                Some(MAX_BUFFER_ERRNO.to_owned()),
                None,
            );
        }
        if timed_out {
            return make(ExecuteFileStatus::Timeout, None, None, Some("SIGTERM".to_owned()));
        }
        match status {
            Some(status) if status.success() => make(ExecuteFileStatus::Ok, Some(0), None, None),
            Some(status) => match status.code() {
                Some(code) => make(ExecuteFileStatus::Exited, Some(code), None, None),
                None => make(ExecuteFileStatus::Killed, None, None, Some("SIGKILL".to_owned())),
            },
            None => make(ExecuteFileStatus::Killed, None, None, None),
        }
    }
}

/// Everything the settings resolver needs from its host.
pub trait SettingsHost:
    ReadFile<Error = io::Error>
    + CheckReadable<Error = io::Error>
    + ReadDirectory<Error = io::Error>
    + ReadSymlinkMetadata<Error = io::Error>
    + ReadLink<Error = io::Error>
    + ExecuteFile
    + HostIdentity
    + super::EnvironmentVariable
    + super::CurrentDirectory<Error = io::Error>
    + super::WallClock<Time = SystemTime>
    + Send
    + Sync
    + 'static
{
}

impl<T> SettingsHost for T where
    T: ReadFile<Error = io::Error>
        + CheckReadable<Error = io::Error>
        + ReadDirectory<Error = io::Error>
        + ReadSymlinkMetadata<Error = io::Error>
        + ReadLink<Error = io::Error>
        + ExecuteFile
        + HostIdentity
        + super::EnvironmentVariable
        + super::CurrentDirectory<Error = io::Error>
        + super::WallClock<Time = SystemTime>
        + Send
        + Sync
        + 'static
{
}
