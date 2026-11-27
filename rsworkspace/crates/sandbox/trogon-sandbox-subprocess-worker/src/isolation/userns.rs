//! What a payload gets in place of the seccomp helper, and the capabilities
//! bubblewrap keeps for the helper.
//!
//! The helper runs unless all Unix sockets are allowed. With no helper the
//! payload runs in bubblewrap's own namespaces with no capability, and
//! bubblewrap is asked for `--disable-userns`, so a user namespace fails with
//! `ENOSPC`, where the helper's filter says `EPERM`. Under the helper a root
//! caller's payload holds a full capability set in the namespace the helper
//! made; see `nested.rs`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;

use trogon_sandbox_contract::LinuxOptions;

use super::seccomp::SeccompOptions;

use super::constants::{
    CAP_SETFCAP_BIT, DISABLE_USERNS_FLAG, NO_USERNS_LIMIT_LOGGED, NO_USERNS_LIMIT_WARNING, SETUID_BIT,
};

/// Whether this bubblewrap lists `--disable-userns` (0.8.0 and later) and can
/// honour it: a setuid bubblewrap refuses the option outright.
pub(super) fn can_disable_userns(bwrap: &Path) -> bool {
    let setuid = std::fs::metadata(bwrap)
        .map(|meta| meta.permissions().mode() & SETUID_BIT != 0)
        .unwrap_or(true);
    if setuid {
        return false;
    }
    let Ok(help) = Command::new(bwrap).arg("--help").output() else {
        return false;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    text.contains(DISABLE_USERNS_FLAG)
}

/// The seccomp filters a launch installs and whether bubblewrap refuses user
/// namespaces in their place.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct NamespaceGuard {
    seccomp: SeccompOptions,
    disable_userns: bool,
    helper: bool,
    leaves_userns_open: bool,
}

impl NamespaceGuard {
    /// Where no helper runs (all Unix sockets allowed without nested user
    /// namespaces), bubblewrap refuses user namespaces and no filter is
    /// installed. When bubblewrap cannot, the namespace filter stays, which is
    /// stricter and shows as `EPERM` rather than `ENOSPC`. Under the weaker
    /// nested sandbox bubblewrap is not asked either.
    pub(super) fn decide(linux: &LinuxOptions, bubblewrap_can_disable_userns: bool) -> Self {
        let seccomp = SeccompOptions::of(linux);
        let helperless = linux.allow_all_unix_sockets.is_allowed() && !linux.allow_nested_user_namespaces.is_allowed();
        let helper = !linux.allow_all_unix_sockets.is_allowed();
        if !helperless {
            return Self {
                seccomp,
                disable_userns: false,
                helper,
                leaves_userns_open: false,
            };
        }
        let everything_allowed = SeccompOptions {
            allow_unix_sockets: true,
            allow_nested_namespaces: true,
        };
        if linux.weaker_nested_sandbox.is_weaker() {
            Self {
                seccomp: everything_allowed,
                disable_userns: false,
                helper,
                leaves_userns_open: true,
            }
        } else if bubblewrap_can_disable_userns {
            Self {
                seccomp: everything_allowed,
                disable_userns: true,
                helper,
                leaves_userns_open: false,
            }
        } else {
            Self {
                seccomp,
                disable_userns: false,
                helper,
                leaves_userns_open: true,
            }
        }
    }

    /// Whether the seccomp helper runs, which is the nested PID namespace of
    /// `nested.rs`.
    pub(super) fn uses_helper(&self) -> bool {
        self.helper
    }

    /// The warning, once per process, where user namespaces stay open. The
    /// namespace filter is still installed unless the weaker nested sandbox
    /// asked for neither.
    pub(super) fn warn_when_userns_open(&self) {
        if self.leaves_userns_open && !NO_USERNS_LIMIT_LOGGED.swap(true, Ordering::Relaxed) {
            tracing::warn!(reason = NO_USERNS_LIMIT_WARNING, "user namespaces left open");
        }
    }

    pub(super) fn seccomp(&self) -> SeccompOptions {
        self.seccomp
    }

    pub(super) fn args(&self) -> Vec<String> {
        self.disable_userns
            .then(|| DISABLE_USERNS_FLAG.to_string())
            .into_iter()
            .collect()
    }
}

/// Everything is dropped; a root caller behind the
/// helper keeps `CAP_SETFCAP` when it holds it, because mapping uid 0 in the
/// helper's user namespace needs it in the creator.
pub(super) fn capability_args(behind_helper: bool, caller_is_root: bool, caller_holds_setfcap: bool) -> Vec<String> {
    let mut args = vec!["--cap-drop".to_string(), "ALL".to_string()];
    if behind_helper && caller_is_root && caller_holds_setfcap {
        args.extend(["--cap-add".to_string(), "CAP_SETFCAP".to_string()]);
    }
    args
}

/// Whether the bounding or inheritable set of this process has
/// `CAP_SETFCAP`.
pub(super) fn holds_setfcap() -> bool {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    ["CapBnd:", "CapInh:"].iter().any(|key| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|value| u64::from_str_radix(value.trim(), 16).ok())
            .is_some_and(|set| set & (1 << CAP_SETFCAP_BIT) != 0)
    })
}

#[cfg(test)]
mod tests;
