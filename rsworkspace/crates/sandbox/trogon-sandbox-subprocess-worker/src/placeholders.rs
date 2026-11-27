use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
struct Ledger {
    active: usize,
    paths: BTreeSet<PathBuf>,
}

#[derive(Clone, Default)]
pub struct PlaceholderLedger {
    state: Arc<Mutex<Ledger>>,
}

impl PlaceholderLedger {
    pub fn shared() -> Self {
        static SHARED: OnceLock<PlaceholderLedger> = OnceLock::new();
        let shared = SHARED.get_or_init(|| {
            let ledger = PlaceholderLedger::default();
            ledger.remove_at_exit();
            ledger
        });
        shared.clone()
    }

    #[cfg(unix)]
    fn remove_at_exit(&self) {
        static AT_EXIT: OnceLock<PlaceholderLedger> = OnceLock::new();
        extern "C" fn sweep() {
            if let Some(ledger) = AT_EXIT.get() {
                ledger.remove_recorded();
            }
        }
        if AT_EXIT.set(self.clone()).is_ok() {
            // SAFETY: `sweep` is a plain `extern "C"` function that lives for
            // the whole process.
            unsafe { libc::atexit(sweep) };
        }
    }

    #[cfg(not(unix))]
    fn remove_at_exit(&self) {}

    fn remove_recorded(&self) {
        let mut state = self.lock();
        state.active = 0;
        let mut paths: Vec<PathBuf> = std::mem::take(&mut state.paths).into_iter().collect();
        paths.sort_by_key(|path| std::cmp::Reverse(path.as_os_str().len()));
        for path in &paths {
            remove_if_still_empty(path);
        }
    }

    pub fn begin(&self) -> PlaceholderHold {
        self.lock().active += 1;
        PlaceholderHold {
            ledger: Some(self.clone()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn give_back(&self) {
        let mut state = self.lock();
        state.active = state.active.saturating_sub(1);
        if state.active > 0 {
            return;
        }
        drop(state);
        self.remove_recorded();
    }
}

pub struct PlaceholderHold {
    ledger: Option<PlaceholderLedger>,
}

impl PlaceholderHold {
    pub fn record(&self, paths: &[PathBuf]) {
        if let Some(ledger) = &self.ledger {
            ledger.lock().paths.extend(paths.iter().cloned());
        }
    }

    pub fn release(mut self) {
        self.release_now();
    }

    pub fn release_now(&mut self) {
        if let Some(ledger) = self.ledger.take() {
            ledger.give_back();
        }
    }
}

impl Drop for PlaceholderHold {
    fn drop(&mut self) {
        self.release_now();
    }
}

fn remove_if_still_empty(path: &std::path::Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_file() && meta.len() == 0 {
        let _ = std::fs::remove_file(path);
    } else if meta.file_type().is_dir() {
        let _ = std::fs::remove_dir(path);
    }
}

#[cfg(test)]
mod tests;
