use std::path::{Path, PathBuf};

use nix::libc;

use super::constants::{HELD, HOOK};

extern "C" fn remove_held() {
    let Ok(held) = HELD.try_lock() else { return };
    remove_all(&held);
}

fn remove_all(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Directories the process made and still holds, removed when the process
/// exits even if nothing got to drop them: `process::exit` runs no
/// destructors and a host may end right after a command.
pub struct ExitGuard {
    path: PathBuf,
}

impl ExitGuard {
    pub fn hold(path: &Path) -> Self {
        HOOK.call_once(|| {
            // SAFETY: `remove_held` is a plain `extern "C"` function with no arguments.
            unsafe { libc::atexit(remove_held) };
        });
        if let Ok(mut held) = HELD.lock() {
            held.push(path.to_path_buf());
        }
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        if let Ok(mut held) = HELD.lock() {
            held.retain(|path| path != &self.path);
        }
    }
}

#[cfg(test)]
mod tests;
