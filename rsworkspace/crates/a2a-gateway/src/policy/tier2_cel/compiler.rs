use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cel_interpreter::Program;

use super::bundle::CelProgramHandle;

/// Failure surface for compiling a CEL rule file. Carries the offending
/// path so audit/operator output can route logs by file without
/// requiring callers to thread the path back through the error
/// separately.
#[derive(Debug, thiserror::Error)]
#[error("{}: {}", path.display(), detail)]
pub struct CelCompileError {
    path: PathBuf,
    detail: Box<str>,
}

impl CelCompileError {
    pub fn for_path(path: impl Into<PathBuf>, detail: impl Into<Box<str>>) -> Self {
        Self {
            path: path.into(),
            detail: detail.into(),
        }
    }
}

pub fn compile_cel_source(source: &str) -> Result<CelProgramHandle, cel_interpreter::ParseErrors> {
    let program = Program::compile(source)?;
    Ok(CelProgramHandle::new(Arc::new(program)))
}

pub fn compile_cel_file(path: &Path) -> Result<(CelProgramHandle, SystemTime), CelCompileError> {
    let source =
        fs::read_to_string(path).map_err(|err| CelCompileError::for_path(path, format!("read failed: {err}")))?;
    let mtime = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map_err(|err| CelCompileError::for_path(path, format!("metadata failed: {err}")))?;
    let program =
        Program::compile(&source).map_err(|err| CelCompileError::for_path(path, format!("compile failed: {err}")))?;
    Ok((CelProgramHandle::new(Arc::new(program)), mtime))
}
