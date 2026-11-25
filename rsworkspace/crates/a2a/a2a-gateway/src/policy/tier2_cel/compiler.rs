use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
            Self::Read { path, .. } | Self::Compile { path, .. } => path,
        }
    }
}

pub fn compile_cel_source(source: &str) -> Result<CelProgramHandle, cel_interpreter::ParseErrors> {
    let program = Program::compile(source)?;
    Ok(CelProgramHandle::new(Arc::new(program)))
}

/// Compile a CEL rule file, returning the program together with the exact
/// source it was compiled from so the bundle can detect rewrites by content.
pub fn compile_cel_file(path: &Path) -> Result<(CelProgramHandle, String), CelCompileError> {
    let source = fs::read_to_string(path).map_err(|source| CelCompileError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let program = compile_cel_path_source(path, &source)?;
    Ok((program, source))
}

pub fn compile_cel_path_source(path: &Path, source: &str) -> Result<CelProgramHandle, CelCompileError> {
    compile_cel_source(source).map_err(|source| CelCompileError::Compile {
        path: path.to_path_buf(),
        source,
    })
}
