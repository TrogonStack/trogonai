//! A command under the Windows network fence: the argv of `trogon-sandbox-fence-helper exec` with
//! the write roots, the read and write denies, the loopback mode and the
//! environment overlay of the command.
//!
//! Filesystem entries are policy paths (see `settings::windows_paths`) and
//! must name a literal path: the fence helper refuses a pattern, and a pattern is
//! not expanded against the disk here.

use trogon_sandbox_contract::{ExtractNoMatch, FsPathEntry, FsPolicy, SecretBinding, SecretMode, SecretTarget};
use trogon_sandbox_protocol::{ExtractError, plan_extract};
use trogon_sandbox_subprocess_worker::windows_fs::{Disk, Mode, RealDisk, apply_fs_policy_on, expand};
use trogon_sandbox_windows_fence::policy_path::grants_beneath_denies_by;
use trogon_sandbox_windows_fence::{
    BinShell, ExecRequest, FsAccessLists, FsAccessSet, WindowsSandboxError, WindowsSandboxErrorCode, rank_fs_access,
};

use crate::command_overrides::CommandOverrides;
use crate::config::PolicyExecConfig;
use crate::constants::SAFE_DIRECTORY_WILDCARD_THRESHOLD;
use crate::error::RuntimeError;
use crate::restrictions::literal_entry;
use crate::settings::windows_paths;

/// What one launch needs besides the config.
#[derive(Debug, Clone)]
pub struct WindowsLaunch {
    pub command: String,
    pub bin_shell: BinShell,
    /// The process whose session the command belongs to.
    pub held_by: Option<u32>,
    /// The broker's `PATH` and `PATHEXT`.
    pub base_env: Vec<(String, String)>,
    /// The sentinels of the `mode: mask` environment variables.
    pub mask_env: Vec<(String, String)>,
    /// The proxy variables of the proxy the command is fenced to; empty when
    /// there is none.
    pub proxy_env: Vec<(String, String)>,
    /// `network.allowLocalBinding`.
    pub allow_loopback: bool,
    /// The directory the command starts in, in the Windows spelling.
    pub cwd: String,
    /// The trust bundle of the proxy's CA, when the proxy terminates TLS.
    pub ca_cert_path: Option<String>,
}

/// The git configuration of a Windows command: `safe.directory` for the
/// working tree and every granted directory, and the OpenSSL backend, since
/// Schannel cannot get credentials under the write-restricted token.
fn git_env(
    directories: &[String],
    schannel_ca: bool,
    openssl_backend: bool,
    base_env: &[(String, String)],
) -> Vec<(String, String)> {
    let existing = base_env
        .iter()
        .find(|(name, _)| name == "GIT_CONFIG_COUNT")
        .map(|(_, value)| value.as_str());
    if existing == Some("0") {
        return Vec::new();
    }
    let start = existing
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut seen = std::collections::BTreeSet::new();
    let unique: Vec<String> = directories
        .iter()
        .map(|dir| {
            let forward = dir.replace('\\', "/");
            let stripped = forward.trim_end_matches('/');
            let bare_root = stripped.is_empty()
                || (stripped.len() == 2
                    && stripped.as_bytes()[0].is_ascii_alphabetic()
                    && stripped.as_bytes()[1] == b':');
            if bare_root {
                format!("{stripped}/")
            } else {
                stripped.to_string()
            }
        })
        .filter(|dir| seen.insert(dir.clone()))
        .collect();
    let mut entries: Vec<(&str, String)> = Vec::new();
    if unique.len() > SAFE_DIRECTORY_WILDCARD_THRESHOLD {
        entries.push(("safe.directory", "*".to_string()));
    } else {
        for dir in unique {
            entries.push(("safe.directory", dir.clone()));
            entries.push((
                "safe.directory",
                if dir.ends_with('/') {
                    format!("{dir}*")
                } else {
                    format!("{dir}/*")
                },
            ));
        }
    }
    if schannel_ca {
        entries.push(("http.schannelUseSSLCAInfo", "true".to_string()));
        entries.push(("http.schannelCheckRevoke", "false".to_string()));
    }
    if openssl_backend {
        entries.push(("http.sslBackend", "openssl".to_string()));
    }
    if entries.is_empty() {
        return Vec::new();
    }
    let mut env = Vec::new();
    for (offset, (key, value)) in entries.iter().enumerate() {
        let index = start + offset;
        env.push((format!("GIT_CONFIG_KEY_{index}"), (*key).to_string()));
        env.push((format!("GIT_CONFIG_VALUE_{index}"), value.clone()));
    }
    env.push(("GIT_CONFIG_COUNT".to_string(), (start + entries.len()).to_string()));
    env
}

/// The `trogon-sandbox-fence-helper exec` request of one command under `config`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn exec_request(config: &PolicyExecConfig, launch: WindowsLaunch) -> Result<ExecRequest, RuntimeError> {
    let policy = if config.configured_platform_options().filesystem.disabled.is_disabled() {
        FsPolicy::default()
    } else {
        config.policy().fs.clone().unwrap_or_default()
    };
    request_for(config, &policy, launch, &RealDisk)
}

fn request_for(
    config: &PolicyExecConfig,
    policy: &FsPolicy,
    launch: WindowsLaunch,
    disk: &dyn Disk,
) -> Result<ExecRequest, RuntimeError> {
    request_with(config, launch, |request| {
        apply_fs_policy_on(request, policy, disk).map_err(|error| RuntimeError::Unsupported(format!("{error:#}")))
    })
}

/// The policy
/// expanded against the disk and ranked into what the session stamps with
/// `trogon-sandbox-fence-helper acl grant`. `credential_deny` are the files the credential
/// bindings keep from the sandbox: a grant at or beneath one is dropped.
/// With `filesystem_disabled` the policy is not applied: only the
/// `credential_deny` files, which the caller limits to the masked ones, stay
/// denied.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn session_access(
    policy: &FsPolicy,
    credential_deny: &[FsPathEntry],
    filesystem_disabled: bool,
    disk: &dyn Disk,
) -> Result<FsAccessSet, RuntimeError> {
    let expanded = |entries: &[FsPathEntry], mode| {
        expand(entries, mode, disk).map_err(|error| RuntimeError::Unsupported(format!("{error:#}")))
    };
    if filesystem_disabled {
        let lists = FsAccessLists {
            deny_read: expanded(credential_deny, Mode::Deny)?,
            ..FsAccessLists::default()
        };
        return Ok(rank_fs_access(&lists, &|path| disk.resolve(path)));
    }
    let lists = FsAccessLists {
        allow_read: expanded(policy.allow_read_within_deny(), Mode::Grant)?,
        allow_write: expanded(policy.allow_write(), Mode::Grant)?,
        deny_read: expanded(policy.deny_read(), Mode::Deny)?,
        deny_write: expanded(policy.deny_write(), Mode::Deny)?,
        credential_deny: expanded(credential_deny, Mode::Deny)?,
        ..FsAccessLists::default()
    };
    Ok(rank_fs_access(&lists, &|path| disk.resolve(path)))
}

/// The files of
/// `mode: "mask"` bindings the sandbox must not read, as an ACL cannot
/// redirect a read. Like the other platforms, a file that does not exist, or
/// that holds no value to mask, is left alone; `read` returns its content.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn masked_file_denies(
    bindings: &[SecretBinding],
    read: &dyn Fn(&str) -> std::io::Result<String>,
) -> Result<Vec<FsPathEntry>, RuntimeError> {
    let mut denies = Vec::new();
    for binding in bindings {
        let SecretTarget::File(path) = binding.target() else {
            continue;
        };
        let denied = match binding.mode() {
            SecretMode::Deny => true,
            SecretMode::Mask => {
                let content = match read(path.as_str()) {
                    Ok(content) => content,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(err) => {
                        return Err(RuntimeError::Invalid(anyhow::anyhow!(
                            "secret binding '{}' cannot read {}: {err}",
                            binding.name(),
                            path.as_str()
                        )));
                    }
                };
                masks_a_value(binding, path.as_str(), &content)?
            }
        };
        if denied {
            let entry = literal_entry(path.as_str()).map_err(|err| RuntimeError::Unsupported(err.to_string()))?;
            if !denies.contains(&entry) {
                denies.push(entry);
            }
        }
    }
    Ok(denies)
}

fn masks_a_value(binding: &SecretBinding, path: &str, content: &str) -> Result<bool, RuntimeError> {
    let Some(rule) = binding.extract() else {
        return Ok(!content.is_empty());
    };
    let failed = |err: &dyn std::fmt::Display| {
        RuntimeError::Invalid(anyhow::anyhow!(
            "secret binding '{}' cannot mask {path}: {err}",
            binding.name()
        ))
    };
    match plan_extract(rule, content, binding.mask_duplicates()) {
        Ok(plan) => Ok(plan.captures().first().is_some_and(|first| !first.is_empty())),
        Err(err @ ExtractError::NoMatch(_)) => match binding.on_extract_no_match() {
            ExtractNoMatch::Error => Err(failed(&err)),
            ExtractNoMatch::Warn => Ok(false),
            ExtractNoMatch::Deny => Ok(true),
        },
        Err(err) => Err(failed(&err)),
    }
}

/// What a session has stamped, which a command run in it is judged against
/// (the filesystem set, the masked files and the verified loopback mode).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct SessionStamp<'a> {
    pub access: &'a FsAccessSet,
    /// The files the command's credentials mask, which stay denied.
    pub masked: &'a [FsPathEntry],
    /// The files the command's own `credentials` section denies.
    pub credential_deny: &'a [FsPathEntry],
    pub verified_loopback: bool,
}

/// The `trogon-sandbox-fence-helper exec` request of a command in a session whose grants and
/// denies are stamped already: it carries the granted write roots and the
/// denies of the command's own `overrides`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn session_request(
    config: &PolicyExecConfig,
    launch: WindowsLaunch,
    session: &SessionStamp<'_>,
    overrides: &CommandOverrides,
    disk: &dyn Disk,
) -> Result<ExecRequest, RuntimeError> {
    check_loopback(overrides, session.verified_loopback)?;
    let denies = per_command_denies(overrides, session, disk)?;
    let mut launch = launch;
    launch.allow_loopback = overrides.allow_local_binding.unwrap_or(launch.allow_loopback);
    let mut config = config.clone();
    let platform = overrides.platform_over(config.configured_platform_options());
    config = config.platform_options(platform);
    request_with(&config, launch, |request| {
        request.write_roots.extend(session.access.grant_write.iter().cloned());
        request.deny_read = denies.deny_read;
        request.deny_read_except = denies.deny_read_except;
        request.deny_write = denies.deny_write;
        Ok(())
    })
}

/// A command may not ask for the other loopback mode than the one the
/// session verified the fence with.
fn check_loopback(overrides: &CommandOverrides, verified: bool) -> Result<(), RuntimeError> {
    let wanted = overrides.allow_local_binding.unwrap_or(verified);
    if wanted == verified {
        return Ok(());
    }
    let mode = |open: bool| if open { "open" } else { "closed" };
    Err(RuntimeError::Launch(
        WindowsSandboxError::new(
            WindowsSandboxErrorCode::WfpLoopbackMismatch,
            format!(
                "network.allowLocalBinding asks for loopback {}, but this session verified the \
                 network fence with loopback {}. On Windows the loopback mode is fixed by the \
                 install and checked at initialize(): it cannot change for one command or \
                 through updateConfig(). Call reset() then initialize() with the new value; the \
                 installed mode must match it.",
                mode(wanted),
                mode(verified)
            ),
        )
        .into(),
    ))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CommandDenies {
    deny_read: Vec<String>,
    deny_read_except: Vec<String>,
    deny_write: Vec<String>,
}

/// The per-exec file denies: those of the command alone, ranked with the
/// session's grants held (a grant cannot be taken back for one command) and
/// left out where the session stamped them already. Grants beneath them rank
/// above them. The command's `filesystem.disabled` skips them all but the
/// masked files.
fn per_command_denies(
    overrides: &CommandOverrides,
    session: &SessionStamp<'_>,
    disk: &dyn Disk,
) -> Result<CommandDenies, RuntimeError> {
    let filesystem = overrides.filesystem.as_ref();
    let disabled = filesystem.is_some_and(|own| own.disabled == Some(true));
    let none: Vec<FsPathEntry> = Vec::new();
    let (raw_read, raw_write, raw_credential) = match filesystem {
        Some(own) if !disabled => {
            if own.has_allows() {
                return Err(RuntimeError::Unsupported(
                    "Per-exec filesystem.allowRead/allowWrite is not supported on Windows - \
                     `trogon-sandbox-fence-helper exec` only exposes per-exec denies. Set them at the session level \
                     (initialize())."
                        .to_string(),
                ));
            }
            (
                own.deny_read.as_ref().unwrap_or(&none),
                own.deny_write.as_ref().unwrap_or(&none),
                session.credential_deny,
            )
        }
        None => (&none, &none, session.credential_deny),
        _ => (&none, &none, &[][..]),
    };
    if raw_read.is_empty() && raw_write.is_empty() && raw_credential.is_empty() && session.masked.is_empty() {
        return Ok(CommandDenies::default());
    }
    let expanded =
        |entries, mode| expand(entries, mode, disk).map_err(|error| RuntimeError::Unsupported(format!("{error:#}")));
    let lists = FsAccessLists {
        deny_read: expanded(raw_read, Mode::Deny)?,
        deny_write: expanded(raw_write, Mode::Deny)?,
        credential_deny: expanded(raw_credential, Mode::Deny)?
            .into_iter()
            .chain(expanded(session.masked, Mode::Deny)?)
            .collect(),
        held: Some(session.access.clone()),
        ..FsAccessLists::default()
    };
    let ranked = rank_fs_access(&lists, &|path| disk.resolve(path));
    let session_read: std::collections::BTreeSet<&String> = session.access.deny_read.iter().collect();
    let session_write: std::collections::BTreeSet<&String> = session.access.deny_write.iter().collect();
    let deny_read: Vec<String> = ranked
        .deny_read
        .into_iter()
        .filter(|path| !session_read.contains(path))
        .collect();
    let deny_write = ranked
        .deny_write
        .into_iter()
        .filter(|path| !session_read.contains(path) && !session_write.contains(path))
        .collect();
    let grants: Vec<String> = session
        .access
        .grant_read
        .iter()
        .chain(&session.access.grant_write)
        .cloned()
        .collect();
    Ok(CommandDenies {
        deny_read_except: grants_beneath_denies_by(&grants, &deny_read, |path| disk.resolve(path)),
        deny_read,
        deny_write,
    })
}

fn request_with(
    config: &PolicyExecConfig,
    launch: WindowsLaunch,
    apply_fs: impl FnOnce(&mut ExecRequest) -> Result<(), RuntimeError>,
) -> Result<ExecRequest, RuntimeError> {
    let mut request = ExecRequest::new(launch.command, launch.bin_shell);
    apply_fs(&mut request)?;
    let mut safe_directories = vec![launch.cwd.clone()];
    safe_directories.extend(request.write_roots.iter().cloned());
    for directory in &config.configured_platform_options().git_safe_directories {
        safe_directories
            .push(windows_paths::native(directory.as_str()).unwrap_or_else(|| directory.as_str().to_string()));
    }

    request.held_by = launch.held_by;
    request.allow_loopback = launch.allow_loopback;
    request.proxy_in_use = !launch.proxy_env.is_empty();
    request.proxy_env = launch.proxy_env;
    request.base_env = launch.base_env.clone();
    request.git_env = git_env(&safe_directories, launch.ca_cert_path.is_some(), true, &launch.mask_env);
    request.mask_env = launch.mask_env;
    Ok(request)
}

#[cfg(test)]
mod tests;
