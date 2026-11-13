use std::ffi::OsStr;

/// Refusal to start a CLI path that Windows would interpret through a shell.
#[derive(Debug, thiserror::Error)]
pub enum CliPathError {
    #[error("Windows batch scripts are not supported; use a native Claude Code executable")]
    WindowsBatchScript,
}

#[cfg(windows)]
pub(crate) fn reject_windows_batch_cli(path: &OsStr) -> Result<(), CliPathError> {
    if is_windows_batch_cli(path) {
        return Err(CliPathError::WindowsBatchScript);
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn reject_windows_batch_cli(_: &OsStr) -> Result<(), CliPathError> {
    Ok(())
}

#[cfg(any(windows, test))]
fn is_windows_batch_cli(path: &OsStr) -> bool {
    path.to_string_lossy().replace('\\', "/").split('/').any(|part| {
        part.split(':').any(|segment| {
            let segment = segment.trim_end_matches(['.', ' ']).to_ascii_lowercase();
            segment.ends_with(".bat") || segment.ends_with(".cmd")
        })
    })
}

#[cfg(test)]
mod tests;
