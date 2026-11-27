//! Which host the runtime runs on, as far as it decides support.

use std::fmt;

/// The version of Windows Subsystem for Linux the process runs under.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct WslVersion(u32);

impl WslVersion {
    /// What `/proc/version` says: an explicit `WSL<n>` marker, or else the
    /// word `microsoft`, which is the original WSL1 format
    /// (`4.4.0-19041-Microsoft`).
    pub fn from_proc_version(text: &str) -> Option<Self> {
        let lower = text.to_ascii_lowercase();
        let marked = lower.match_indices("wsl").find_map(|(at, marker)| {
            let digits: String = lower[at + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok().map(Self)
        });
        marked.or_else(|| lower.contains("microsoft").then_some(Self(1)))
    }

    /// The version this process runs under, `None` outside WSL and off Linux.
    pub fn detect() -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        std::fs::read_to_string("/proc/version")
            .ok()
            .and_then(|text| Self::from_proc_version(&text))
    }

    pub fn get(self) -> u32 {
        self.0
    }

    /// WSL1 has no namespaces, so bubblewrap cannot run there.
    pub fn is_supported(self) -> bool {
        self.0 != 1
    }
}

impl fmt::Display for WslVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub(crate) fn is_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos")) && WslVersion::detect().is_none_or(WslVersion::is_supported)
}

#[cfg(test)]
mod tests;
