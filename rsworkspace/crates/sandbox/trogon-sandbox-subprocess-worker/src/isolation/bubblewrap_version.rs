use std::cmp::Ordering;
use std::fmt;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use super::constants::{BUSY_BACKOFF, BUSY_RETRIES};

/// A dotted numeric `bwrap --version`, compared component by component with
/// missing components counting as zero.
#[derive(Debug, Clone, Eq)]
pub struct BubblewrapVersion(Vec<u64>);

impl BubblewrapVersion {
    /// Before 0.5.0 a file bind mount point had to be a regular file.
    pub fn oldest_supported() -> Self {
        Self(vec![0, 5, 0])
    }

    /// Finds the first run of dotted digits in the output of `bwrap --version`.
    pub fn parse(output: &str) -> Option<Self> {
        let start = output.find(|c: char| c.is_ascii_digit())?;
        let dotted: String = output[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        dotted
            .trim_end_matches('.')
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<Vec<u64>>>()
            .map(Self)
    }

    pub fn probe(bwrap: &Path) -> Option<Self> {
        let output = Self::run_version(bwrap)?;
        if !output.status.success() {
            return None;
        }
        Self::parse(&String::from_utf8_lossy(&output.stdout))
    }

    fn run_version(bwrap: &Path) -> Option<std::process::Output> {
        for _ in 0..BUSY_RETRIES {
            match Command::new(bwrap).arg("--version").output() {
                Ok(output) => return Some(output),
                Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => std::thread::sleep(BUSY_BACKOFF),
                Err(_) => return None,
            }
        }
        None
    }

    pub fn is_outdated(&self) -> bool {
        *self < Self::oldest_supported()
    }

    /// An older bubblewrap still launches. It only cannot bind a special file
    /// (a fifo, socket or device node) over a path, so the launch warns.
    pub fn warn_if_outdated(bwrap: &Path) {
        let Some(found) = Self::probe(bwrap) else {
            tracing::warn!(
                bwrap = %bwrap.display(),
                "could not determine the bubblewrap version, continuing"
            );
            return;
        };
        if found.is_outdated() {
            tracing::warn!(
                bwrap = %bwrap.display(),
                %found,
                oldest = %Self::oldest_supported(),
                "bubblewrap is older than the version that can bind special files, continuing"
            );
        }
    }
}

impl PartialEq for BubblewrapVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Ord for BubblewrapVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.0.len().max(other.0.len());
        (0..len)
            .map(|i| {
                let a = self.0.get(i).copied().unwrap_or(0);
                let b = other.0.get(i).copied().unwrap_or(0);
                a.cmp(&b)
            })
            .find(|ordering| ordering.is_ne())
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for BubblewrapVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for BubblewrapVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.0.iter().map(u64::to_string).collect();
        f.write_str(&parts.join("."))
    }
}

#[cfg(test)]
mod tests;
