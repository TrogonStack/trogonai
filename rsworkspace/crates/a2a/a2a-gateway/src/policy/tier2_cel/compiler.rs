use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cel_interpreter::Program;

use super::bundle::CelProgramHandle;

/// Failure surface for compiling a CEL rule file. Each variant carries
/// the typed source so callers / audit can serialize the root cause
/// without flattening it to a string at this layer.
#[derive(Debug, thiserror::Error)]
pub enum CelCompileError {
    #[error("read cel file {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read cel file metadata {}", path.display())]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("compile cel file {}", path.display())]
    Compile {
        path: PathBuf,
        #[source]
        source: cel_interpreter::ParseErrors,
    },
}

impl CelCompileError {
    pub fn path(&self) -> &Path {
        match self {
            Self::Read { path, .. } | Self::Metadata { path, .. } | Self::Compile { path, .. } => path,
        }
    }
}

pub fn compile_cel_source(source: &str) -> Result<CelProgramHandle, cel_interpreter::ParseErrors> {
    let program = Program::compile(source)?;
    Ok(CelProgramHandle::new(Arc::new(program)))
}

pub fn compile_cel_file(path: &Path) -> Result<(CelProgramHandle, SystemTime), CelCompileError> {
    // Capture mtime BEFORE reading the source. If we read first and then
    // stat, a writer that races us between the two syscalls would leave
    // us with old contents stamped with the new mtime, which then
    // mismatches the cached mtime on the next refresh and keeps us
    // stuck on stale rules until another mtime change.
    let mtime = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map_err(|source| CelCompileError::Metadata {
            path: path.to_path_buf(),
            source,
        })?;
    let source = fs::read_to_string(path).map_err(|source| CelCompileError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let program = Program::compile(&source).map_err(|source| CelCompileError::Compile {
        path: path.to_path_buf(),
        source,
    })?;
    Ok((CelProgramHandle::new(Arc::new(program)), mtime))
}
