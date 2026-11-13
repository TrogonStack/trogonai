#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use trogon_claude_agent_sdk::adapters::{
    CheckReadable, CurrentDirectory, DirectoryEntry, DirectoryEntryKind, EnvironmentVariable, ExecuteFile,
    ExecuteFileOutcome, ExecuteFileRequest, ExecuteFileStatus, HostIdentity, OperatingSystem, ReadDirectory, ReadFile,
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
    pub commands: BTreeMap<String, ExecuteFileOutcome>,
    pub runs: Mutex<Vec<String>>,
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
            commands: BTreeMap::new(),
            runs: Mutex::new(Vec::new()),
        }
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
        self.commands
            .get(&line)
            .cloned()
            .unwrap_or_else(|| spawn_error("ENOENT"))
    }
}
