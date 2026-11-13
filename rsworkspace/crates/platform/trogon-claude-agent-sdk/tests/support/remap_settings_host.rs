#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use trogon_claude_agent_sdk::adapters::{
    CheckReadable, CheckWritable, CurrentDirectory, DirectoryEntry, EffectiveGroupId, EffectiveUserId,
    EnvironmentVariable, ExecuteFile, ExecuteFileOutcome, ExecuteFileRequest, HostIdentity, LinkMetadata, NativeHost,
    OperatingSystem, ReadDirectory, ReadFile, ReadLink, ReadMetadata, ReadSymlinkMetadata, RealGroupId, RealUserId,
    SupplementaryGroupIds, WallClock,
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
    /// Milliseconds since the epoch the host's clock shows.
    pub now_ms: u64,
    /// The process ids the case pins; `None` keeps the real ones.
    pub identity: Option<ProcessIdentity>,
    /// Write checks the case answers, by the path the resolver asks about (`*` answers every other path): `yes`
    /// succeeds and an errno name such as `EACCES` fails. Paths without an answer get the native check.
    pub writable: BTreeMap<String, String>,
}

/// The ids a process reports; a `None` field is a call the platform lacks.
#[derive(Clone, Debug, Default)]
pub struct ProcessIdentity {
    pub euid: Option<u32>,
    pub uid: Option<u32>,
    pub egid: Option<u32>,
    pub gid: Option<u32>,
    pub groups: Option<Vec<u32>>,
}

/// The error kind Rust gives the errno a case names.
fn errno_error(name: &str) -> io::Error {
    io::Error::from(match name {
        "EACCES" | "EPERM" => io::ErrorKind::PermissionDenied,
        "EROFS" => io::ErrorKind::ReadOnlyFilesystem,
        "ENOENT" => io::ErrorKind::NotFound,
        "ENOTDIR" => io::ErrorKind::NotADirectory,
        _ => io::ErrorKind::Other,
    })
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

impl ReadMetadata for RemapHost {
    type Error = io::Error;

    fn metadata(&self, path: &Path) -> Result<std::fs::Metadata, io::Error> {
        NativeHost.metadata(&self.remap(path))
    }
}

impl CheckWritable for RemapHost {
    type Error = io::Error;

    fn check_writable(&self, path: &Path) -> Result<(), io::Error> {
        let asked = path.to_string_lossy();
        match self.writable.get(&*asked).or_else(|| self.writable.get("*")) {
            Some(answer) if answer == "yes" => Ok(()),
            Some(answer) => Err(errno_error(answer)),
            None => NativeHost.check_writable(&self.remap(path)),
        }
    }
}

impl EffectiveUserId for RemapHost {
    fn effective_user_id(&self) -> Option<u32> {
        match &self.identity {
            Some(identity) => identity.euid,
            None => NativeHost.effective_user_id(),
        }
    }
}

impl RealUserId for RemapHost {
    fn real_user_id(&self) -> Option<u32> {
        match &self.identity {
            Some(identity) => identity.uid,
            None => NativeHost.real_user_id(),
        }
    }
}

impl EffectiveGroupId for RemapHost {
    fn effective_group_id(&self) -> Option<u32> {
        match &self.identity {
            Some(identity) => identity.egid,
            None => NativeHost.effective_group_id(),
        }
    }
}

impl RealGroupId for RemapHost {
    fn real_group_id(&self) -> Option<u32> {
        match &self.identity {
            Some(identity) => identity.gid,
            None => NativeHost.real_group_id(),
        }
    }
}

impl SupplementaryGroupIds for RemapHost {
    type Error = io::Error;

    fn supplementary_group_ids(&self) -> Option<Result<Vec<u32>, io::Error>> {
        match &self.identity {
            Some(identity) => identity.groups.clone().map(Ok),
            None => NativeHost.supplementary_group_ids(),
        }
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

impl WallClock for RemapHost {
    type Time = SystemTime;

    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.now_ms)
    }
}
