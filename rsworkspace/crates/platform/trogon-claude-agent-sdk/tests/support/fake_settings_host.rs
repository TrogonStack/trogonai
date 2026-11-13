#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use trogon_claude_agent_sdk::adapters::{
    CheckReadable, CheckWritable, CurrentDirectory, DirectoryEntry, DirectoryEntryKind, EffectiveGroupId,
    EffectiveUserId, EnvironmentVariable, ExecuteFile, ExecuteFileOutcome, ExecuteFileRequest, ExecuteFileStatus,
    HomeDirectory, HostIdentity, LinkMetadata, OpenDirectoryNoFollow, OperatingSystem, ReadDeviceId, ReadDirectory,
    ReadFile, ReadFileHead, ReadLink, ReadMetadata, ReadSymlinkMetadata, RealGroupId, RealUserId,
    SupplementaryGroupIds, WallClock,
};

/// A host made of maps, so every platform branch runs on any machine.
pub struct FakeHost {
    pub system: OperatingSystem,
    pub user: Option<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub files: BTreeMap<PathBuf, Vec<u8>>,
    pub directories: BTreeMap<PathBuf, Vec<DirectoryEntry>>,
    pub unreadable: BTreeSet<PathBuf>,
    pub links: BTreeMap<PathBuf, PathBuf>,
    pub commands: BTreeMap<String, ExecuteFileOutcome>,
    /// Every command line run, shared with the copies a background read takes.
    pub runs: Arc<Mutex<Vec<String>>>,
    /// Milliseconds since the epoch the host's clock shows. Commands leave it alone: the resolver counts the
    /// `duration_ms` they report, the way the oracle's fake clock counts a fixture's `durations`.
    pub clock_ms: AtomicU64,
    /// While set, every command waits before it answers, shared with the copies a background read takes.
    pub held: Arc<(Mutex<bool>, Condvar)>,
    /// Whether running a command fails the read the way a host that throws does.
    pub throws: bool,
}

/// A copy sees the files and commands as they are now and the clock where it stands now, and records its runs
/// with the original.
impl Clone for FakeHost {
    fn clone(&self) -> Self {
        Self {
            system: self.system.clone(),
            user: self.user.clone(),
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            files: self.files.clone(),
            directories: self.directories.clone(),
            unreadable: self.unreadable.clone(),
            links: self.links.clone(),
            commands: self.commands.clone(),
            runs: Arc::clone(&self.runs),
            clock_ms: AtomicU64::new(self.clock_ms.load(Ordering::SeqCst)),
            held: Arc::clone(&self.held),
            throws: self.throws,
        }
    }
}

impl FakeHost {
    pub fn linux() -> Self {
        Self {
            system: OperatingSystem::Linux,
            user: Some("fixture-user".to_owned()),
            cwd: PathBuf::from("/work/project"),
            env: BTreeMap::from([("HOME".to_owned(), "/home/fixture".to_owned())]),
            files: BTreeMap::new(),
            directories: BTreeMap::new(),
            unreadable: BTreeSet::new(),
            links: BTreeMap::new(),
            commands: BTreeMap::new(),
            runs: Arc::new(Mutex::new(Vec::new())),
            clock_ms: AtomicU64::new(0),
            held: Arc::new((Mutex::new(false), Condvar::new())),
            throws: false,
        }
    }

    /// Makes every command wait until [`FakeHost::release_commands`].
    pub fn hold_commands(&self) {
        *self.held.0.lock().expect("held") = true;
    }

    pub fn release_commands(&self) {
        *self.held.0.lock().expect("held") = false;
        self.held.1.notify_all();
    }

    /// Sets the clock, in milliseconds since the epoch.
    pub fn at(self, milliseconds: u64) -> Self {
        self.clock_ms.store(milliseconds, Ordering::SeqCst);
        self
    }

    pub fn set_time(&self, milliseconds: u64) {
        self.clock_ms.store(milliseconds, Ordering::SeqCst);
    }

    pub fn file(mut self, path: &str, text: &str) -> Self {
        self.files.insert(PathBuf::from(path), text.as_bytes().to_vec());
        self
    }

    pub fn directory(mut self, path: &str, names: &[&str]) -> Self {
        self.directories.insert(
            PathBuf::from(path),
            names
                .iter()
                .map(|name| DirectoryEntry {
                    name: (*name).to_owned(),
                    kind: DirectoryEntryKind::File,
                })
                .collect(),
        );
        self
    }

    /// A symbolic link at `path` whose text is `target`.
    pub fn link(mut self, path: &str, target: &str) -> Self {
        self.links.insert(PathBuf::from(path), PathBuf::from(target));
        self
    }

    pub fn variable(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_owned(), value.to_owned());
        self
    }

    pub fn command(mut self, line: &str, outcome: ExecuteFileOutcome) -> Self {
        self.commands.insert(line.to_owned(), outcome);
        self
    }

    pub fn ran(&self) -> Vec<String> {
        self.runs.lock().expect("runs").clone()
    }
}

pub fn ok(stdout: &str) -> ExecuteFileOutcome {
    ExecuteFileOutcome {
        stdout: stdout.to_owned(),
        stderr: String::new(),
        status: ExecuteFileStatus::Ok,
        exit_code: Some(0),
        errno: None,
        signal: None,
        duration_ms: 0,
    }
}

pub fn exited(code: i32, stderr: &str) -> ExecuteFileOutcome {
    ExecuteFileOutcome {
        stdout: String::new(),
        stderr: stderr.to_owned(),
        status: ExecuteFileStatus::Exited,
        exit_code: Some(code),
        errno: None,
        signal: None,
        duration_ms: 0,
    }
}

pub fn spawn_error(errno: &str) -> ExecuteFileOutcome {
    ExecuteFileOutcome {
        stdout: String::new(),
        stderr: String::new(),
        status: ExecuteFileStatus::SpawnError,
        exit_code: None,
        errno: Some(errno.to_owned()),
        signal: None,
        duration_ms: 0,
    }
}

fn not_found() -> io::Error {
    io::Error::from(io::ErrorKind::NotFound)
}

impl ReadFile for FakeHost {
    type Error = io::Error;

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, io::Error> {
        if self.unreadable.contains(path) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        self.files.get(path).cloned().ok_or_else(not_found)
    }
}

impl CheckReadable for FakeHost {
    type Error = io::Error;

    fn check_readable(&self, path: &Path) -> Result<(), io::Error> {
        if self.unreadable.contains(path) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        if self.files.contains_key(path) {
            Ok(())
        } else {
            Err(not_found())
        }
    }
}

impl ReadDirectory for FakeHost {
    type Error = io::Error;

    fn read_directory(&self, path: &Path) -> Result<Vec<DirectoryEntry>, io::Error> {
        self.directories.get(path).cloned().ok_or_else(not_found)
    }
}

impl ReadSymlinkMetadata for FakeHost {
    type Error = io::Error;

    fn symlink_metadata(&self, path: &Path) -> Result<LinkMetadata, io::Error> {
        if let Some(target) = self.links.get(path) {
            return Ok(LinkMetadata {
                kind: DirectoryEntryKind::Symlink,
                len: target.as_os_str().len() as u64,
            });
        }
        if let Some(bytes) = self.files.get(path) {
            return Ok(LinkMetadata {
                kind: DirectoryEntryKind::File,
                len: bytes.len() as u64,
            });
        }
        let holds_something = self
            .files
            .keys()
            .chain(self.links.keys())
            .chain(self.directories.keys())
            .any(|held| held.starts_with(path));
        if holds_something {
            return Ok(LinkMetadata {
                kind: DirectoryEntryKind::Directory,
                len: 0,
            });
        }
        Err(not_found())
    }
}

impl ReadLink for FakeHost {
    type Error = io::Error;

    fn read_link(&self, path: &Path) -> Result<PathBuf, io::Error> {
        self.links
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
    }
}

/// The maps hold no real files, so there is no native metadata to return.
impl ReadMetadata for FakeHost {
    type Error = io::Error;

    fn metadata(&self, _path: &Path) -> Result<std::fs::Metadata, io::Error> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

impl CheckWritable for FakeHost {
    type Error = io::Error;

    fn check_writable(&self, _path: &Path) -> Result<(), io::Error> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// The process ids of a platform without them, so nothing asks the maps for ownership.
impl EffectiveUserId for FakeHost {
    fn effective_user_id(&self) -> Option<u32> {
        None
    }
}

impl RealUserId for FakeHost {
    fn real_user_id(&self) -> Option<u32> {
        None
    }
}

impl EffectiveGroupId for FakeHost {
    fn effective_group_id(&self) -> Option<u32> {
        None
    }
}

impl RealGroupId for FakeHost {
    fn real_group_id(&self) -> Option<u32> {
        None
    }
}

impl SupplementaryGroupIds for FakeHost {
    type Error = io::Error;

    fn supplementary_group_ids(&self) -> Option<Result<Vec<u32>, io::Error>> {
        None
    }
}

impl OpenDirectoryNoFollow for FakeHost {
    type Error = io::Error;

    fn open_directory_no_follow(&self, path: &Path) -> Result<(), io::Error> {
        match self.symlink_metadata(path)?.kind {
            DirectoryEntryKind::Directory => Ok(()),
            _ => Err(io::Error::from(io::ErrorKind::NotADirectory)),
        }
    }
}

impl ReadFileHead for FakeHost {
    type Error = io::Error;

    fn read_file_head(&self, path: &Path, limit: usize) -> Result<Vec<u8>, io::Error> {
        if self.links.contains_key(path) {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let mut bytes = self.read_file(path)?;
        bytes.truncate(limit);
        Ok(bytes)
    }
}

/// No folder of the fake host sits on a device of its own.
impl ReadDeviceId for FakeHost {
    type Error = io::Error;

    fn device_id(&self, _path: &Path) -> Result<u64, io::Error> {
        Err(not_found())
    }
}

impl HomeDirectory for FakeHost {
    type Error = io::Error;

    fn home_directory(&self) -> Result<PathBuf, io::Error> {
        self.env.get("HOME").map(PathBuf::from).ok_or_else(not_found)
    }
}

impl HostIdentity for FakeHost {
    fn operating_system(&self) -> OperatingSystem {
        self.system.clone()
    }

    fn username(&self) -> Option<String> {
        self.user.clone()
    }
}

impl EnvironmentVariable for FakeHost {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        self.env.get(key.to_str()?).map(OsString::from)
    }
}

impl CurrentDirectory for FakeHost {
    type Error = io::Error;

    fn current_dir(&self) -> Result<PathBuf, io::Error> {
        Ok(self.cwd.clone())
    }
}

impl ExecuteFile for FakeHost {
    fn execute_file(&self, request: &ExecuteFileRequest<'_>) -> ExecuteFileOutcome {
        let mut line = request.program.to_string_lossy().into_owned();
        for argument in request.args {
            line.push(' ');
            line.push_str(&argument.to_string_lossy());
        }
        self.runs.lock().expect("runs").push(line.clone());
        assert!(!self.throws, "the host throws while running {line}");
        let (held, released) = &*self.held;
        drop(
            released
                .wait_while(held.lock().expect("held"), |holding| *holding)
                .expect("held"),
        );
        self.commands
            .get(&line)
            .cloned()
            .unwrap_or_else(|| spawn_error("ENOENT"))
    }
}

impl WallClock for FakeHost {
    type Time = SystemTime;

    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.clock_ms.load(Ordering::SeqCst))
    }
}
