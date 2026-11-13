#![allow(
    dead_code,
    reason = "The shared decorator supports independent adapter contract tests."
)]

//! A decorator over [`NativeFileSystem`] rooted in a temporary directory.
//!
//! Every adapter call is recorded and then delegated to the native call, so
//! the runtime under test performs real filesystem effects. Tests may inject
//! an `Err` return in place of chosen calls.

use std::{
    fs::{DirBuilder, File, Metadata, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use tempfile::TempDir;
use trogon_claude_agent_sdk::adapters::{
    CanonicalizePath, CreateDirectory, NativeFileSystem, OpenFile, ReadMetadata, RemoveDirectoryAll, RemoveFile,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Canonicalize,
    Open,
    CreateDirectory,
    Metadata,
    RemoveDirectoryAll,
    RemoveFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub operation: Operation,
    pub path: PathBuf,
}

struct Fault {
    operation: Operation,
    suffix: Option<PathBuf>,
    kind: io::ErrorKind,
    skip: usize,
    remaining: usize,
}

#[derive(Default)]
struct State {
    calls: Vec<Call>,
    faults: Vec<Fault>,
}

#[derive(Clone)]
pub struct RecordingFileSystem {
    directory: Arc<TempDir>,
    root: PathBuf,
    state: Arc<Mutex<State>>,
}

impl RecordingFileSystem {
    pub fn new() -> io::Result<Self> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        Ok(Self {
            directory: Arc::new(directory),
            root,
            state: Arc::new(Mutex::new(State::default())),
        })
    }

    /// Resolves a path relative to the temporary root.
    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }

    fn lock(&self) -> io::Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("filesystem recording poisoned"))
    }

    pub fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    pub fn write_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)
    }

    pub fn exists(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    pub fn calls(&self) -> io::Result<Vec<Call>> {
        Ok(self.lock()?.calls.clone())
    }

    /// Paths passed to `operation`, in call order.
    pub fn paths(&self, operation: Operation) -> io::Result<Vec<PathBuf>> {
        Ok(self
            .lock()?
            .calls
            .iter()
            .filter(|call| call.operation == operation)
            .map(|call| call.path.clone())
            .collect())
    }

    /// Returns `kind` from the next `times` calls of `operation` whose path ends with `suffix`, or from any path when
    /// `suffix` is `None`.
    pub fn fail(
        &self,
        operation: Operation,
        suffix: Option<&str>,
        kind: io::ErrorKind,
        times: usize,
    ) -> io::Result<()> {
        self.fail_after(operation, suffix, kind, 0, times)
    }

    /// Like [`Self::fail`], but lets the first `skip` matching calls through.
    pub fn fail_after(
        &self,
        operation: Operation,
        suffix: Option<&str>,
        kind: io::ErrorKind,
        skip: usize,
        times: usize,
    ) -> io::Result<()> {
        self.lock()?.faults.push(Fault {
            operation,
            suffix: suffix.map(PathBuf::from),
            kind,
            skip,
            remaining: times,
        });
        Ok(())
    }

    fn record(&self, operation: Operation, path: &Path) -> io::Result<()> {
        let mut state = self.lock()?;
        state.calls.push(Call {
            operation,
            path: path.to_path_buf(),
        });
        let fault = state.faults.iter_mut().find(|fault| {
            fault.remaining > 0
                && fault.operation == operation
                && fault.suffix.as_ref().is_none_or(|suffix| path.ends_with(suffix))
        });
        if let Some(fault) = fault {
            if fault.skip > 0 {
                fault.skip -= 1;
                return Ok(());
            }
            fault.remaining -= 1;
            return Err(io::Error::from(fault.kind));
        }
        Ok(())
    }
}

impl CanonicalizePath for RecordingFileSystem {
    type Error = io::Error;
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.record(Operation::Canonicalize, path)?;
        NativeFileSystem.canonicalize(path)
    }
}

impl OpenFile for RecordingFileSystem {
    type Error = io::Error;
    fn open(&self, options: &OpenOptions, path: &Path) -> io::Result<File> {
        self.record(Operation::Open, path)?;
        NativeFileSystem.open(options, path)
    }
}

impl CreateDirectory for RecordingFileSystem {
    type Error = io::Error;
    fn create_directory(&self, builder: &DirBuilder, path: &Path) -> io::Result<()> {
        self.record(Operation::CreateDirectory, path)?;
        NativeFileSystem.create_directory(builder, path)
    }
}

impl ReadMetadata for RecordingFileSystem {
    type Error = io::Error;
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        self.record(Operation::Metadata, path)?;
        NativeFileSystem.metadata(path)
    }
}

impl RemoveDirectoryAll for RecordingFileSystem {
    type Error = io::Error;
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.record(Operation::RemoveDirectoryAll, path)?;
        NativeFileSystem.remove_dir_all(path)
    }
}

impl RemoveFile for RecordingFileSystem {
    type Error = io::Error;
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.record(Operation::RemoveFile, path)?;
        NativeFileSystem.remove_file(path)
    }
}
