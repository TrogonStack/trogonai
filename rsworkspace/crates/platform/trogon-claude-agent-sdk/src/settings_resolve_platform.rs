use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    adapters::{
        EnvironmentVariable, ExecuteFile, ExecuteFileOutcome, ExecuteFileRequest, ExecuteFileStatus, OperatingSystem,
        ReadFile, SettingsHost,
    },
    constants::{BUSY_ERRNOS, MAX_BUFFER_ERRNO},
    constants::{
        LINUX_MANAGED_DIRECTORY, MACOS_MANAGED_DIRECTORY, PLIST_DIRECTORY, PLIST_DOMAIN, PLUTIL_CONVERT_ARGS,
        PLUTIL_LINT_ARGS, PLUTIL_PATH, POLICY_READ_MAX_BUFFER, POLICY_READ_TIMEOUT, PROC_VERSION_PATH,
        REGISTRY_MACHINE_KEY, REGISTRY_NOT_FOUND_TEXT, REGISTRY_USER_KEY, REGISTRY_VALUE_NAME,
        WINDOWS_MANAGED_DIRECTORY, WSL_MANAGED_DIRECTORY, WSL_REGISTRY_TOOL,
    },
    settings_resolve_tier::{LoadState, TierRead},
    settings_resolve_values::SettingsMap,
};

/// Platform names `getPlatform()` upstream returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlatformKind {
    Macos,
    Windows,
    Wsl,
    Linux,
    Unknown,
}

impl PlatformKind {
    /// The name `getPlatform()` upstream returns.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Wsl => "wsl",
            Self::Linux => "linux",
            Self::Unknown => "unknown",
        }
    }
}

fn variable_is_set<H: EnvironmentVariable>(host: &H, name: &str) -> bool {
    host.var_os(OsStr::new(name)).is_some_and(|value| !value.is_empty())
}

fn kernel_looks_like_wsl(text: &str) -> bool {
    let lowered = text.to_lowercase();
    lowered.contains("microsoft") || lowered.contains("wsl")
}

fn proc_version<H: ReadFile<Error = io::Error>>(host: &H) -> Option<String> {
    host.read_file(Path::new(PROC_VERSION_PATH))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// `Cie().getPlatform()` upstream.
pub(crate) fn platform_kind<H: SettingsHost>(host: &H) -> PlatformKind {
    match host.operating_system() {
        OperatingSystem::Darwin => PlatformKind::Macos,
        OperatingSystem::Win32 => PlatformKind::Windows,
        OperatingSystem::Linux => {
            if variable_is_set(host, "WSL_DISTRO_NAME")
                || variable_is_set(host, "WSL_INTEROP")
                || proc_version(host).is_some_and(|text| kernel_looks_like_wsl(&text))
            {
                PlatformKind::Wsl
            } else {
                PlatformKind::Linux
            }
        }
        OperatingSystem::Other(_) => PlatformKind::Unknown,
    }
}

/// `Gc()` upstream: a Linux process that runs under WSL.
pub(crate) fn runs_under_wsl<H: SettingsHost>(host: &H) -> bool {
    if host.operating_system() != OperatingSystem::Linux {
        return false;
    }
    if variable_is_set(host, "WSL_DISTRO_NAME") {
        return true;
    }
    proc_version(host).is_some_and(|text| kernel_looks_like_wsl(&text))
}

/// `LYe.getManagedFilePath()` upstream.
pub(crate) fn managed_directory(kind: PlatformKind) -> PathBuf {
    PathBuf::from(match kind {
        PlatformKind::Macos => MACOS_MANAGED_DIRECTORY,
        PlatformKind::Windows => WINDOWS_MANAGED_DIRECTORY,
        _ => LINUX_MANAGED_DIRECTORY,
    })
}

/// Directory that holds the Windows managed settings as WSL reaches it.
pub(crate) fn wsl_managed_directory() -> PathBuf {
    PathBuf::from(WSL_MANAGED_DIRECTORY)
}

/// How one registry run ended (`Eq` upstream).
#[derive(Clone, Debug)]
pub(crate) struct CommandOutcome {
    pub(crate) status: ExecuteFileStatus,
    pub(crate) exit_code: Option<i32>,
    pub(crate) errno: Option<String>,
    pub(crate) answered: bool,
}

impl CommandOutcome {
    fn from_run(run: &ExecuteFileOutcome) -> Self {
        let answered = run.status == ExecuteFileStatus::Ok
            || (run.status == ExecuteFileStatus::Exited
                && run.exit_code == Some(1)
                && run.stderr.contains(REGISTRY_NOT_FOUND_TEXT));
        Self {
            status: run.status,
            exit_code: run.exit_code,
            errno: run.errno.clone(),
            answered,
        }
    }

    /// `Ey` upstream: the machine did not answer and the failure is not a plain "no such key".
    pub(crate) fn lost_the_answer(&self) -> bool {
        let plain_miss = self.status == ExecuteFileStatus::Exited && self.exit_code == Some(1);
        let hard_spawn_failure = self.status == ExecuteFileStatus::SpawnError
            && self.errno.as_deref().is_none_or(|errno| !BUSY_ERRNOS.contains(&errno));
        !(self.answered || plain_miss || hard_spawn_failure)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PlistDocument {
    pub(crate) stdout: Option<String>,
}

/// What the host's policy readers produced (`by()` upstream).
#[derive(Clone, Debug, Default)]
pub(crate) struct MdmRead {
    pub(crate) plists: Option<Vec<PlistDocument>>,
    pub(crate) hklm_stdout: Option<String>,
    pub(crate) hklm_oversize: bool,
    pub(crate) hkcu_stdout: Option<String>,
    pub(crate) hklm_outcome: Option<CommandOutcome>,
}

fn run<H: ExecuteFile>(host: &H, program: &OsStr, args: &[&OsStr], max_buffer: Option<usize>) -> ExecuteFileOutcome {
    host.execute_file(&ExecuteFileRequest {
        program,
        args,
        timeout: POLICY_READ_TIMEOUT,
        max_buffer,
    })
}

fn finished(run: &ExecuteFileOutcome) -> bool {
    run.status == ExecuteFileStatus::Ok || (run.status == ExecuteFileStatus::Exited && run.exit_code.is_some())
}

fn is_absent_error(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

fn read_plist<H: SettingsHost>(host: &H, path: &Path) -> Option<PlistDocument> {
    if let Err(error) = host.check_readable(path) {
        if is_absent_error(&error) {
            return None;
        }
        return Some(PlistDocument { stdout: None });
    }
    let program = OsStr::new(PLUTIL_PATH);
    let mut convert: Vec<&OsStr> = PLUTIL_CONVERT_ARGS.iter().map(OsStr::new).collect();
    convert.push(path.as_os_str());
    let converted = run(host, program, &convert, Some(POLICY_READ_MAX_BUFFER));
    if !finished(&converted) {
        return Some(PlistDocument { stdout: None });
    }
    if converted.status == ExecuteFileStatus::Ok {
        return Some(PlistDocument {
            stdout: Some(converted.stdout),
        });
    }
    let mut lint: Vec<&OsStr> = PLUTIL_LINT_ARGS.iter().map(OsStr::new).collect();
    lint.push(path.as_os_str());
    let linted = run(host, program, &lint, None);
    if !finished(&linted) || linted.status == ExecuteFileStatus::Ok {
        return Some(PlistDocument { stdout: None });
    }
    match host.check_readable(path) {
        Ok(()) => Some(PlistDocument {
            stdout: Some(String::new()),
        }),
        Err(error) if is_absent_error(&error) => None,
        Err(_) => Some(PlistDocument { stdout: None }),
    }
}

fn read_registry<H: SettingsHost>(host: &H, tool: &OsStr) -> MdmRead {
    let query = |key: &str| {
        let args = [
            OsStr::new("query"),
            OsStr::new(key),
            OsStr::new("/v"),
            OsStr::new(REGISTRY_VALUE_NAME),
        ];
        run(host, tool, &args, Some(POLICY_READ_MAX_BUFFER))
    };
    let machine = query(REGISTRY_MACHINE_KEY);
    let user = query(REGISTRY_USER_KEY);
    MdmRead {
        plists: None,
        hklm_stdout: (machine.status == ExecuteFileStatus::Ok).then(|| machine.stdout.clone()),
        hklm_oversize: machine.status == ExecuteFileStatus::SpawnError
            && machine.errno.as_deref() == Some(MAX_BUFFER_ERRNO),
        hkcu_stdout: (user.status == ExecuteFileStatus::Ok).then(|| user.stdout.clone()),
        hklm_outcome: Some(CommandOutcome::from_run(&machine)),
    }
}

/// `by()` upstream: ask the operating system for its managed policy.
pub(crate) fn read_policy<H: SettingsHost>(host: &H) -> MdmRead {
    match host.operating_system() {
        OperatingSystem::Darwin => {
            let mut candidates: Vec<PathBuf> = Vec::new();
            if let Some(username) = host.username().filter(|name| !name.is_empty()) {
                candidates.push(
                    Path::new(PLIST_DIRECTORY)
                        .join(username)
                        .join(format!("{PLIST_DOMAIN}.plist")),
                );
            }
            candidates.push(Path::new(PLIST_DIRECTORY).join(format!("{PLIST_DOMAIN}.plist")));
            let mut documents: Vec<PlistDocument> = Vec::new();
            for path in candidates {
                if let Some(document) = read_plist(host, &path) {
                    documents.push(document);
                }
            }
            if let Some(first) = documents.iter().position(|document| document.stdout.is_some()) {
                documents.truncate(first + 1);
            }
            MdmRead {
                plists: Some(documents),
                ..MdmRead::default()
            }
        }
        OperatingSystem::Win32 => {
            let root = host
                .var_os(OsStr::new("SYSTEMROOT"))
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| "C:\\Windows".to_owned());
            let tool = format!("{root}\\System32\\reg.exe");
            read_registry(host, OsStr::new(&tool))
        }
        _ if runs_under_wsl(host) => read_registry(host, OsStr::new(WSL_REGISTRY_TOOL)),
        _ => MdmRead::default(),
    }
}

/// `hE` upstream: the `Settings` value out of `reg query` output.
pub(crate) fn registry_value(stdout: &str) -> Option<String> {
    let mut line_starts = vec![0usize];
    for (index, character) in stdout.char_indices() {
        if matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            line_starts.push(index + character.len_utf8());
        }
    }
    for start in line_starts {
        if let Some(value_start) = registry_value_start(&stdout[start..]).map(|offset| start + offset) {
            let value = stdout[value_start..].trim_end();
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}

fn skip_blanks(text: &str) -> Option<&str> {
    let rest = text.trim_start_matches([' ', '\t']);
    (rest.len() < text.len()).then_some(rest)
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

fn registry_value_start(line: &str) -> Option<usize> {
    let rest = skip_blanks(line)?;
    let rest = strip_prefix_ignore_case(rest, REGISTRY_VALUE_NAME)?;
    let rest = skip_blanks(rest)?;
    let rest = strip_prefix_ignore_case(rest, "REG_")?;
    let rest = strip_prefix_ignore_case(rest, "EXPAND_").unwrap_or(rest);
    let rest = strip_prefix_ignore_case(rest, "SZ")?;
    let rest = skip_blanks(rest)?;
    Some(line.len() - rest.len())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WslFlag {
    Armed,
    Disarmed,
    Unreadable,
}

/// `oP` upstream.
pub(crate) fn wsl_flag(document: &SettingsMap) -> WslFlag {
    match document.get("wslInheritsWindowsSettings") {
        None | Some(Value::Null) => WslFlag::Disarmed,
        Some(Value::Bool(true)) => WslFlag::Armed,
        Some(Value::Bool(false)) => WslFlag::Disarmed,
        Some(Value::String(text)) if text == "true" => WslFlag::Armed,
        Some(Value::String(text)) if text == "false" => WslFlag::Disarmed,
        Some(_) => WslFlag::Unreadable,
    }
}

/// Result of the Windows-side managed settings walk (`yet` upstream).
pub(crate) struct WindowsSideFlag {
    pub(crate) flag: WslFlag,
    pub(crate) load_state: LoadState,
    pub(crate) unreadable: bool,
}

fn drop_in_name_is_eligible(name: &str) -> bool {
    name.ends_with(".json") && !name.starts_with('.')
}

fn windows_side_document<H: SettingsHost>(
    host: &H,
    path: &Path,
    state: &mut LoadState,
    unreadable: &mut bool,
) -> WslFlag {
    let bytes = match host.read_file(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            if !is_absent_error(&error) {
                *unreadable = true;
                *state = LoadState::DidNotLoad;
            }
            return WslFlag::Disarmed;
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let parsed = if text.trim().is_empty() {
        Some(SettingsMap::new())
    } else {
        serde_json::from_str::<Value>(&text).ok().and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
    };
    match parsed {
        None => {
            *state = LoadState::DidNotLoad;
            WslFlag::Disarmed
        }
        Some(document) => {
            *state = state.combine(LoadState::Loaded);
            wsl_flag(&document)
        }
    }
}

pub(crate) fn read_windows_side<H: SettingsHost>(host: &H) -> WindowsSideFlag {
    let root = wsl_managed_directory();
    let mut state = LoadState::Absent;
    let mut unreadable = false;
    let file = root.join(crate::constants::MANAGED_SETTINGS_FILE);
    let mut flag = windows_side_document(host, &file, &mut state, &mut unreadable);
    let result = |flag, state, unreadable| WindowsSideFlag {
        flag,
        load_state: state,
        unreadable,
    };
    if flag == WslFlag::Armed {
        return result(flag, state, unreadable);
    }
    let directory = root.join(crate::constants::MANAGED_SETTINGS_DROP_IN_DIRECTORY);
    let entries = match host.read_directory(&directory) {
        Ok(entries) => entries,
        Err(error) => {
            if !is_absent_error(&error) {
                state = LoadState::DidNotLoad;
                unreadable = true;
            }
            return result(flag, state, unreadable);
        }
    };
    for entry in entries {
        let eligible = matches!(
            entry.kind,
            crate::adapters::DirectoryEntryKind::File | crate::adapters::DirectoryEntryKind::Symlink
        ) && drop_in_name_is_eligible(&entry.name);
        if !eligible {
            continue;
        }
        let path = directory.join(&entry.name);
        let found = windows_side_document(host, &path, &mut state, &mut unreadable);
        if found == WslFlag::Armed {
            return result(found, state, unreadable);
        }
        if found == WslFlag::Unreadable {
            flag = found;
        }
    }
    result(flag, state, unreadable)
}

/// Whether a managed settings document exists in `directory` (`nD` upstream).
pub(crate) fn directory_holds_managed_settings<H: SettingsHost>(host: &H, directory: &Path) -> bool {
    let exists = |path: &Path| match host.read_file(path) {
        Ok(_) => true,
        Err(error) => !is_absent_error(&error),
    };
    if exists(&directory.join(crate::constants::MANAGED_SETTINGS_FILE)) {
        return true;
    }
    let drop_ins = directory.join(crate::constants::MANAGED_SETTINGS_DROP_IN_DIRECTORY);
    match host.read_directory(&drop_ins) {
        Err(error) => !is_absent_error(&error),
        Ok(entries) => entries.iter().any(|entry| {
            matches!(
                entry.kind,
                crate::adapters::DirectoryEntryKind::File | crate::adapters::DirectoryEntryKind::Symlink
            ) && drop_in_name_is_eligible(&entry.name)
                && exists(&drop_ins.join(&entry.name))
        }),
    }
}

/// What the readers evaluate to (`_E` upstream).
pub(crate) struct MdmEvaluation {
    pub(crate) mdm: TierRead,
    pub(crate) hkcu: SettingsMap,
    pub(crate) wsl_inherits: bool,
}

/// `_E` upstream: turn the raw reads into the machine policy tier.
pub(crate) fn evaluate_policy<H: SettingsHost>(host: &H, read: &MdmRead) -> MdmEvaluation {
    let mut state = LoadState::Absent;
    let mut lost = false;
    let mut unreadable = false;
    for document in read.plists.iter().flatten() {
        let parsed = match &document.stdout {
            None => TierRead::did_not_load(true),
            Some(stdout) => TierRead::from_hm(stdout, false),
        };
        if parsed.has_policy_content {
            return MdmEvaluation {
                mdm: parsed,
                hkcu: SettingsMap::new(),
                wsl_inherits: false,
            };
        }
        lost |= parsed.lo();
        unreadable |= parsed.unreadable;
        state = state.combine(parsed.load_state);
    }
    let mut machine: Option<TierRead> = None;
    let mut flag = WslFlag::Disarmed;
    if let Some(stdout) = &read.hklm_stdout {
        let text = registry_value(stdout).unwrap_or_default();
        machine = Some(TierRead::from_hm(&text, false));
        if let Some(Value::Object(document)) = serde_json::from_str::<Value>(&text).ok().filter(|_| !text.is_empty()) {
            flag = wsl_flag(&document);
        }
    } else if read.hklm_oversize {
        machine = Some(TierRead::did_not_load(true));
    }
    let machine_lost = machine.as_ref().is_some_and(TierRead::lo);
    if let Some(machine) = &machine {
        unreadable |= machine.unreadable;
        state = state.combine(machine.load_state);
    }
    let under_wsl = runs_under_wsl(host);
    let mut path_flag = flag;
    if under_wsl {
        path_flag = flag;
        if path_flag != WslFlag::Armed {
            let side = read_windows_side(host);
            if side.flag == WslFlag::Armed || path_flag == WslFlag::Disarmed {
                path_flag = side.flag;
            }
            if path_flag == WslFlag::Disarmed {
                unreadable |= side.unreadable;
                state = state.combine(side.load_state);
            }
        }
    }
    let inherits = path_flag != WslFlag::Disarmed;
    let base = || TierRead {
        settings: Some(SettingsMap::new()),
        has_policy_content: false,
        load_state: state,
        user_writable: false,
        unreadable,
    };
    let _ = lost;
    if under_wsl && !inherits {
        return MdmEvaluation {
            mdm: base(),
            hkcu: SettingsMap::new(),
            wsl_inherits: false,
        };
    }
    if let Some(machine) = machine
        && machine_lost
    {
        return MdmEvaluation {
            mdm: machine,
            hkcu: SettingsMap::new(),
            wsl_inherits: inherits,
        };
    }
    if path_flag == WslFlag::Unreadable {
        let mut tier = base();
        tier.has_policy_content = true;
        tier.load_state = tier.load_state.combine(LoadState::Loaded);
        return MdmEvaluation {
            mdm: tier,
            hkcu: SettingsMap::new(),
            wsl_inherits: inherits,
        };
    }
    if let Some(stdout) = &read.hkcu_stdout {
        let held = (inherits && directory_holds_managed_settings(host, &wsl_managed_directory()))
            || directory_holds_managed_settings(host, &managed_directory(platform_kind(host)));
        if !held {
            let text = registry_value(stdout).unwrap_or_default();
            let user = TierRead::from_hm(&text, true);
            let settings = user.settings.clone().unwrap_or_default();
            let opted_in = settings.get("wslInheritsWindowsSettings") == Some(&Value::Bool(true));
            if !under_wsl || opted_in {
                let mut settings = settings;
                settings.shift_remove("wslInheritsWindowsSettings");
                settings.shift_remove(crate::constants::MANAGED_SOURCES_BEHAVIOR_KEY);
                return MdmEvaluation {
                    mdm: base(),
                    hkcu: settings,
                    wsl_inherits: inherits,
                };
            }
        }
    }
    MdmEvaluation {
        mdm: base(),
        hkcu: SettingsMap::new(),
        wsl_inherits: inherits,
    }
}
