#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use trogon_claude_agent_sdk::adapters::{
    CheckReadable, CurrentDirectory, DirectoryEntry, EnvironmentVariable, ExecuteFile, ExecuteFileOutcome,
    ExecuteFileRequest, HostIdentity, LinkMetadata, NativeHost, OperatingSystem, ReadDirectory, ReadFile, ReadLink,
    ReadSymlinkMetadata, WallClock,
};

/// Machine-wide locations the resolver reads, which the host moves under a sandbox root.
const VIRTUAL: [&str; 6] = [
    "/etc/claude-code",
    "/Library/Application Support/ClaudeCode",
    "/Library/Managed Preferences",
    "/mnt/c",
    "/proc/version",
    "/usr/bin/plutil",
];

/// The real host with machine-wide paths moved under `root`, the platform pinned and the environment overridden.
#[derive(Clone)]
pub struct RemapHost {
    pub root: PathBuf,
    pub system: OperatingSystem,
    /// `None` hides an inherited variable.
    pub env: BTreeMap<String, Option<String>>,
    /// Milliseconds since the epoch the host's clock shows.
    pub now_ms: u64,
    /// Milliseconds each command line (before the paths move under `root`) reports having taken; any other line
    /// reports none, so the outcome never depends on how fast the machine is.
    pub durations: BTreeMap<String, u64>,
}

impl RemapHost {
    fn remap(&self, path: &Path) -> PathBuf {
        for prefix in VIRTUAL {
            if let Ok(rest) = path.strip_prefix(prefix) {
                return self.root.join(prefix.trim_start_matches('/')).join(rest);
            }
        }
        path.to_path_buf()
    }
}

impl ReadFile for RemapHost {
    type Error = io::Error;

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, io::Error> {
        NativeHost.read_file(&self.remap(path))
    }
}

impl CheckReadable for RemapHost {
    type Error = io::Error;

    fn check_readable(&self, path: &Path) -> Result<(), io::Error> {
        NativeHost.check_readable(&self.remap(path))
    }
}

impl ReadDirectory for RemapHost {
    type Error = io::Error;

    fn read_directory(&self, path: &Path) -> Result<Vec<DirectoryEntry>, io::Error> {
        NativeHost.read_directory(&self.remap(path))
    }
}

impl ReadSymlinkMetadata for RemapHost {
    type Error = io::Error;

    fn symlink_metadata(&self, path: &Path) -> Result<LinkMetadata, io::Error> {
        NativeHost.symlink_metadata(&self.remap(path))
    }
}

impl ReadLink for RemapHost {
    type Error = io::Error;

    fn read_link(&self, path: &Path) -> Result<PathBuf, io::Error> {
        NativeHost.read_link(&self.remap(path))
    }
}

impl HostIdentity for RemapHost {
    fn operating_system(&self) -> OperatingSystem {
        self.system.clone()
    }

    fn username(&self) -> Option<String> {
        NativeHost.username()
    }
}

impl EnvironmentVariable for RemapHost {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        match key.to_str().and_then(|name| self.env.get(name)) {
            Some(value) => value.as_ref().map(OsString::from),
            None => NativeHost.var_os(key),
        }
    }
}

impl CurrentDirectory for RemapHost {
    type Error = io::Error;

    fn current_dir(&self) -> Result<PathBuf, io::Error> {
        NativeHost.current_dir()
    }
}

impl ExecuteFile for RemapHost {
    fn execute_file(&self, request: &ExecuteFileRequest<'_>) -> ExecuteFileOutcome {
        let mut line = request.program.to_string_lossy().into_owned();
        for argument in request.args {
            line.push(' ');
            line.push_str(&argument.to_string_lossy());
        }
        let program = self.remap(Path::new(request.program));
        let arguments: Vec<OsString> = request
            .args
            .iter()
            .map(|argument| self.remap(Path::new(argument)).into_os_string())
            .collect();
        let borrowed: Vec<&OsStr> = arguments.iter().map(OsString::as_os_str).collect();
        let mut outcome = NativeHost.execute_file(&ExecuteFileRequest {
            program: program.as_os_str(),
            args: &borrowed,
            timeout: request.timeout,
            max_buffer: request.max_buffer,
        });
        outcome.duration_ms = u128::from(self.durations.get(&line).copied().unwrap_or(0));
        outcome
    }
}

impl WallClock for RemapHost {
    type Time = SystemTime;

    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.now_ms)
    }
}
