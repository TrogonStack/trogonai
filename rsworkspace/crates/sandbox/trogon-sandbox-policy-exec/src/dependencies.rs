#[cfg(any(target_os = "linux", test))]
use std::os::unix::fs::PermissionsExt;
#[cfg(any(target_os = "linux", test))]
use std::process::{Command, Stdio};
#[cfg(any(target_os = "linux", test))]
use std::time::{Duration, Instant};

#[cfg(unix)]
use trogon_sandbox_contract::FsPathEntry;
use trogon_sandbox_protocol::LiteralPathExt;
use trogon_sandbox_subprocess_worker::IsolationBackendKind;
#[cfg(target_os = "linux")]
use trogon_sandbox_subprocess_worker::isolation::landlock::LandlockSupport;

use crate::config::PolicyExecConfig;
#[cfg(any(target_os = "linux", test))]
use crate::constants::{CAP_SETFCAP, CAP_SETFCAP_MISSING_MESSAGE, OLDEST_FULLY_SUPPORTED_BWRAP_VERSION, PROBE_TIMEOUT};

/// What the host lacks for the configured backend. Errors stop a run;
/// warnings name protections that will be weaker than asked for.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DependencyCheck {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl DependencyCheck {
    pub fn of(config: &PolicyExecConfig) -> Self {
        let mut check = Self::default();
        if !crate::platform::is_supported() {
            check.errors.push("Unsupported platform".to_string());
            return check;
        }
        match config.backend_kind() {
            IsolationBackendKind::None => {}
            #[cfg(target_os = "linux")]
            IsolationBackendKind::Bubblewrap => check.linux(config),
            #[cfg(target_os = "macos")]
            IsolationBackendKind::Seatbelt => check.macos(),
            #[allow(unreachable_patterns)]
            _ => check.errors.push("Unsupported platform".to_string()),
        }
        #[cfg(unix)]
        if config.backend_kind() != IsolationBackendKind::None {
            check.own_install(config);
        }
        check
    }

    #[cfg(unix)]
    fn own_install(&mut self, config: &PolicyExecConfig) {
        if config.configured_platform_options().filesystem.disabled.is_disabled() {
            return;
        }
        let Some(restriction) = config.fs_write_restriction() else {
            return;
        };
        let executable = match config.helper_path() {
            Some(helper) => helper.to_path_buf(),
            None => match std::env::current_exe() {
                Ok(executable) => executable,
                Err(_) => return,
            },
        };
        if let Some(warning) = own_install_warning(restriction.roots(), &executable) {
            self.warnings.push(warning);
        }
    }

    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    #[cfg(target_os = "linux")]
    fn linux(&mut self, config: &PolicyExecConfig) {
        let bwrap = match resolve_bwrap(config.bwrap_path(), || which::which("bwrap").ok()) {
            Ok(path) => Some(path),
            Err(error) => {
                self.errors.push(error);
                None
            }
        };
        if let Some(socat) = config.socat_path()
            && !is_executable_file(socat)
        {
            self.errors.push(format!("socat not executable at {}", socat.display()));
        }
        if !matches!(LandlockSupport::detect(), LandlockSupport::Full(_)) {
            self.warnings
                .push("landlock not fully available - filesystem rules are enforced by mounts only".to_string());
        }
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        self.capability_findings(
            &status,
            seccomp_available(),
            bwrap.as_deref(),
            probe_uid0_user_namespace_once,
            probe_bwrap_version,
        );
    }

    #[cfg(any(target_os = "linux", test))]
    fn capability_findings(
        &mut self,
        status: &str,
        seccomp_available: bool,
        bwrap: Option<&std::path::Path>,
        probe: impl FnOnce(&std::path::Path) -> Option<String>,
        version: impl FnOnce(&std::path::Path) -> Option<String>,
    ) {
        if !seccomp_available {
            self.warnings
                .push("seccomp not available - unix socket access not restricted".to_string());
        }
        if let Some(bwrap) = bwrap {
            let has_setfcap =
                bounding_capabilities_from_status(status).is_some_and(|bits| bits >> CAP_SETFCAP & 1 == 1);
            if let Some(error) = uid0_sandbox_error(effective_uid(status), has_setfcap, bwrap, probe) {
                self.errors.push(error);
            }
            if let Some(warning) = outdated_bwrap_warning(bwrap, version(bwrap)) {
                self.warnings.push(warning);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn macos(&mut self) {
        if !std::path::Path::new("/usr/bin/sandbox-exec").is_file() {
            self.errors
                .push("sandbox-exec not found at /usr/bin/sandbox-exec".to_string());
        }
    }
}

/// A warning when a sandboxed command could rewrite the executable the host
/// runs unsandboxed for it (judged on the
/// copy the host process loads): the helper binary when one is set,
/// the running executable otherwise, as the isolation backends re-run it.
///
/// That is when `executable` lies at or under a literal write root, as
/// spelled or where it really is. None otherwise, and for a build output of a
/// cargo project (a `target` directory beside a `Cargo.toml`), which is the
/// checkout being worked on. A write root that is a pattern is not judged.
#[cfg(unix)]
fn own_install_warning(
    write_roots: &[trogon_sandbox_contract::FsPathEntry],
    executable: &std::path::Path,
) -> Option<String> {
    let executable = if executable.is_absolute() {
        executable.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(executable)
    };
    let real = crate::restrictions::real_path_of(&executable).ok();
    if is_cargo_build_output(real.as_deref().unwrap_or(&executable)) {
        return None;
    }
    let own = spellings(&executable, real.as_deref());
    let covered = write_roots.iter().any(|root| {
        let FsPathEntry::Literal { path, .. } = root else {
            return false;
        };
        let root_real = crate::restrictions::real_path_of(path.as_str()).ok();
        spellings(std::path::Path::new(path.as_str()), root_real.as_deref())
            .iter()
            .any(|allowed| own.iter().any(|form| form.is_within(allowed)))
    });
    covered.then(|| {
        format!(
            "{} is run outside the sandbox, where a sandboxed command can write it - install it outside the allowed write paths",
            executable.display()
        )
    })
}

#[cfg(unix)]
fn spellings(path: &std::path::Path, real: Option<&std::path::Path>) -> Vec<trogon_sandbox_contract::LiteralPath> {
    let mut forms = Vec::new();
    for candidate in std::iter::once(path).chain(real) {
        if let Some(form) = candidate
            .to_str()
            .and_then(|text| trogon_sandbox_contract::LiteralPath::new(text).ok())
            && !forms.contains(&form)
        {
            forms.push(form);
        }
    }
    forms
}

#[cfg(unix)]
fn is_cargo_build_output(path: &std::path::Path) -> bool {
    path.ancestors().any(|dir| {
        dir.file_name().is_some_and(|name| name == "target")
            && dir.parent().is_some_and(|project| project.join("Cargo.toml").is_file())
    })
}

#[cfg(any(target_os = "linux", test))]
fn resolve_bwrap(
    explicit: Option<&std::path::Path>,
    on_path: impl FnOnce() -> Option<std::path::PathBuf>,
) -> Result<std::path::PathBuf, String> {
    match explicit {
        Some(path) => {
            if is_executable_file(path) {
                Ok(path.to_path_buf())
            } else {
                Err(format!("bubblewrap (bwrap) not executable at {}", path.display()))
            }
        }
        None => on_path().ok_or_else(|| "bubblewrap (bwrap) not installed".to_string()),
    }
}

#[cfg(any(target_os = "linux", test))]
fn is_executable_file(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(any(target_os = "linux", test))]
fn probe_once(
    cache: &std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Option<String>>>,
    bwrap: &std::path::Path,
    probe: impl FnOnce(&std::path::Path) -> Option<String>,
) -> Option<String> {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = cache.get(bwrap) {
        return known.clone();
    }
    let answer = probe(bwrap);
    cache.insert(bwrap.to_path_buf(), answer.clone());
    answer
}

#[cfg(target_os = "linux")]
fn probe_uid0_user_namespace_once(bwrap: &std::path::Path) -> Option<String> {
    static CACHE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Option<String>>>> =
        std::sync::LazyLock::new(Default::default);
    probe_once(&CACHE, bwrap, |bwrap| probe_uid0_user_namespace(bwrap, PROBE_TIMEOUT))
}

/// CapBnd unioned with CapInh, which is what an exec gives a uid 0 caller's
/// child. `None` when either line is absent.
#[cfg(any(target_os = "linux", test))]
fn bounding_capabilities_from_status(status: &str) -> Option<u64> {
    let field = |name: &str| {
        status.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.trim();
            u64::from_str_radix(value, 16).ok()
        })
    };
    Some(field("CapBnd:")? | field("CapInh:")?)
}

#[cfg(any(target_os = "linux", test))]
fn effective_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// A uid 0 caller without CAP_SETFCAP cannot sandbox anything on a kernel
/// that enforces it, so `probe` asks bubblewrap before this fails the caller.
/// `probe` returns bubblewrap's refusal, empty when it could not be run, or
/// `None` when the uid map is accepted.
#[cfg(any(target_os = "linux", test))]
fn uid0_sandbox_error(
    euid: Option<u32>,
    has_setfcap: bool,
    bwrap: &std::path::Path,
    probe: impl FnOnce(&std::path::Path) -> Option<String>,
) -> Option<String> {
    if euid != Some(0) || has_setfcap {
        return None;
    }
    let refusal = probe(bwrap)?;
    Some(if refusal.is_empty() {
        CAP_SETFCAP_MISSING_MESSAGE.to_string()
    } else {
        format!("{CAP_SETFCAP_MISSING_MESSAGE} (bubblewrap: {refusal})")
    })
}

#[cfg(any(target_os = "linux", test))]
fn probe_uid0_user_namespace(bwrap: &std::path::Path, timeout: std::time::Duration) -> Option<String> {
    let child = Command::new(bwrap)
        .args(["--unshare-user", "--dev-bind", "/", "/", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return Some(String::new());
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    if status.is_some_and(|status| status.success()) {
        return None;
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    Some(
        stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(200).collect())
            .unwrap_or_default(),
    )
}

#[cfg(target_os = "linux")]
fn probe_bwrap_version(bwrap: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(bwrap)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let version: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    Some(version)
}

/// Negative when `a` is the older version. A missing or unreadable component
/// counts as 0.
#[cfg(any(target_os = "linux", test))]
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |version: &str| -> Vec<u64> { version.split('.').map(|part| part.parse().unwrap_or(0)).collect() };
    let (left, right) = (parts(a), parts(b));
    (0..left.len().max(right.len()))
        .map(|index| {
            let l = left.get(index).copied().unwrap_or(0);
            let r = right.get(index).copied().unwrap_or(0);
            l.cmp(&r)
        })
        .find(|ordering| ordering.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

#[cfg(any(target_os = "linux", test))]
fn outdated_bwrap_warning(bwrap: &std::path::Path, version: Option<String>) -> Option<String> {
    let version = version?;
    if compare_versions(&version, OLDEST_FULLY_SUPPORTED_BWRAP_VERSION).is_ge() {
        return None;
    }
    Some(format!(
        "bubblewrap {version} at {} is older than {OLDEST_FULLY_SUPPORTED_BWRAP_VERSION}: a denyRead entry or credential mask naming a path that is not a regular file (a fifo, socket or device node) cannot be applied on it. Everything else is unaffected",
        bwrap.display()
    ))
}

#[cfg(target_os = "linux")]
fn seccomp_available() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .is_ok_and(|status| status.lines().any(|line| line.starts_with("Seccomp:")))
}

#[cfg(test)]
mod tests;
