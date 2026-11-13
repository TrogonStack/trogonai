#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

use trogon_claude_agent_sdk::adapters::{
    CheckReadable, CurrentDirectory, DirectoryEntry, EnvironmentVariable, ExecuteFile, ExecuteFileOutcome,
    ExecuteFileRequest, HostIdentity, NativeHost, OperatingSystem, ReadDirectory, ReadFile,
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
pub struct RemapHost {
    pub root: PathBuf,
    pub system: OperatingSystem,
    /// `None` hides an inherited variable.
    pub env: BTreeMap<String, Option<String>>,
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
        let program = self.remap(Path::new(request.program));
        let arguments: Vec<OsString> = request
            .args
            .iter()
            .map(|argument| self.remap(Path::new(argument)).into_os_string())
            .collect();
        let borrowed: Vec<&OsStr> = arguments.iter().map(OsString::as_os_str).collect();
        NativeHost.execute_file(&ExecuteFileRequest {
            program: program.as_os_str(),
            args: &borrowed,
            timeout: request.timeout,
            max_buffer: request.max_buffer,
        })
    }
}
