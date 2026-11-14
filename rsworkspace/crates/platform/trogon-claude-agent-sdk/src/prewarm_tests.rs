use std::collections::BTreeMap;

use super::*;

fn object(text: &str) -> JsonObject {
    serde_json::from_str(text).unwrap_or_default()
}

fn text(value: &JsonObject) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[test]
fn settings_split_into_what_precedes_and_what_follows_the_claim() {
    let split = split_settings(&object(
        r#"{"model":"x","permissions":{"allow":["Read"]},"env":{"K":"v"},"agent":"a","extra":1}"#,
    ));
    assert_eq!(
        split.before.as_ref().map(text).as_deref(),
        Some(r#"{"env":{"K":"v"},"extra":1}"#)
    );
    // The following keys are sent in upstream's order, not the order they were given in.
    assert_eq!(
        split.after.as_ref().map(text).as_deref(),
        Some(r#"{"model":"x","agent":"a","permissions":{"allow":["Read"]}}"#)
    );
    let all = split_settings(&object(
        r#"{"disableAutoMode":"d","availableModels":[],"permissions":{},"fastMode":true,"agent":"a","model":"m"}"#,
    ));
    assert_eq!(all.before, None);
    assert_eq!(
        all.after.as_ref().map(text).as_deref(),
        Some(
            r#"{"model":"m","agent":"a","fastMode":true,"permissions":{},"availableModels":[],"disableAutoMode":"d"}"#
        )
    );
    assert_eq!(split_settings(&JsonObject::new()), SettingsSplit::default());
}

#[test]
fn hook_settings_are_the_keys_a_claim_cannot_apply() {
    let found = hook_settings(&object(
        r#"{"hooks":{},"model":"m","allowedHttpHookUrls":[],"httpHookAllowedEnvVars":[],"disableAllHooks":false,"allowManagedHooksOnly":true}"#,
    ));
    assert_eq!(
        found,
        [
            "hooks",
            "disableAllHooks",
            "allowManagedHooksOnly",
            "allowedHttpHookUrls",
            "httpHookAllowedEnvVars"
        ]
    );
    assert!(hook_settings(&object(r#"{"model":"m"}"#)).is_empty());
}

#[test]
fn claim_directory_resolves_only_plain_relative_paths() -> std::io::Result<()> {
    let current = || -> std::io::Result<PathBuf> { Ok(PathBuf::from("/work")) };
    let resolve = |path: &str| claim_directory(Path::new(path), &current);
    assert_eq!(resolve("rel/dir")?, "/work/rel/dir");
    assert_eq!(resolve("./a/../b")?, "/work/b");
    assert_eq!(resolve("../x")?, "/x");
    for untouched in ["/abs/dir", "~", "~/proj", "C:\\win", "\\\\share", "", "  "] {
        assert_eq!(resolve(untouched)?, untouched);
    }
    Ok(())
}

#[test]
fn claim_directory_reports_an_unreadable_process_directory() {
    let failing = || -> std::io::Result<PathBuf> { Err(std::io::Error::other("gone")) };
    assert!(claim_directory(Path::new("rel"), &failing).is_err());
    assert!(claim_directory(Path::new("/abs"), &failing).is_ok());
}

#[test]
fn the_claim_request_keeps_upstream_key_order() -> Result<(), Error> {
    let mut agents = BTreeMap::new();
    agents.insert(
        AgentName::from("r"),
        AgentDefinition::new("d", crate::Prompt::from("p")),
    );
    let mut options = ClaimOptions::new("/s");
    options.permission_mode = Some(PermissionMode::Plan);
    options.env = Some(vec![("B".into(), "2".into()), ("A".into(), "1".into())]);
    options.additional_directories = Some(vec!["/x".into(), "/y".into()]);
    options.append_system_prompt = Some("tail".into());
    options.title = Some("T".into());
    options.agents = Some(agents);
    options.workspace_trust = Some(WorkspaceTrust {
        accepted: Some(true),
        directory: Some("/trusted".into()),
    });
    let request = claim_request("/s", &options, &["alpha".to_owned()])?;
    assert_eq!(
        text(&request),
        r#"{"subtype":"claim_session","cwd":"/s","permission_mode":"plan","env":{"B":"2","A":"1"},"additional_directories":["/x","/y"],"append_system_prompt":"tail","title":"T","agents":{"r":{"description":"d","prompt":"p"}},"workspace_trust":{"accepted":true,"directory":"/trusted"},"sdk_mcp_servers":["alpha"],"include_initialize":true}"#
    );
    let bare = claim_request("/s", &ClaimOptions::new("/s"), &[])?;
    assert_eq!(
        text(&bare),
        r#"{"subtype":"claim_session","cwd":"/s","sdk_mcp_servers":[],"include_initialize":true}"#
    );
    Ok(())
}

#[test]
fn workspace_trust_is_sent_only_when_accepted_with_an_absolute_directory() -> Result<(), Error> {
    for trust in [
        WorkspaceTrust {
            accepted: Some(true),
            directory: Some("relative".into()),
        },
        WorkspaceTrust {
            accepted: Some(false),
            directory: Some("/t".into()),
        },
        WorkspaceTrust::default(),
    ] {
        let mut options = ClaimOptions::new("/s");
        options.workspace_trust = Some(trust);
        assert!(!text(&claim_request("/s", &options, &[])?).contains("workspace_trust"));
    }
    Ok(())
}

#[test]
fn claim_errors_carry_the_upstream_messages() {
    let claim = ClaimInfo {
        cwd: "/s".into(),
        session_id: "id".into(),
        parked_ms: Some(1.0),
        sdk_mcp_settled: true,
    };
    assert_eq!(
        ClaimError::AlreadyClaimed.to_string(),
        "SpareProcess.claim() can only be called once"
    );
    assert_eq!(
        ClaimError::Unavailable.to_string(),
        "SpareProcess.claim(): the spare was closed or has exited; start the session with query()"
    );
    assert_eq!(
        ClaimError::SpareExited.to_string(),
        "spare_exited: the spare process exited before it was claimed"
    );
    assert_eq!(
        ClaimError::SpareClosed.to_string(),
        "spare_closed: the spare was closed before it was claimed"
    );
    let hooks = ClaimError::HookSettings {
        keys: vec!["hooks".into(), "disableAllHooks".into()],
    };
    assert_eq!(
        hooks.to_string(),
        "SpareProcess.claim(): hook settings (hooks, disableAllHooks) cannot be applied at claim; pass them to prewarm() in Options.settings, where they are in force before any hook, as on a cold start. Nothing was sent: this spare can still be claimed without them."
    );
    assert_eq!(
        hooks.telemetry_message(),
        Some("SpareProcess.claim(): hook settings cannot be applied at claim")
    );
    let settings = ClaimError::SettingsNotApplied {
        reason: "boom".into(),
        claim: claim.clone(),
    };
    assert_eq!(
        settings.to_string(),
        "settings_not_applied: boom \u{2014} the prompt was not sent and the process was closed; start the session with query()"
    );
    assert_eq!(settings.telemetry_message(), Some("settings_not_applied"));
    assert_eq!(settings.claim(), Some(&claim));
    let options = ClaimError::OptionNotApplied {
        reasons: vec!["model: a".into(), "maxThinkingTokens: b".into()],
        claim,
    };
    assert_eq!(
        options.to_string(),
        "option_not_applied: model: a; maxThinkingTokens: b \u{2014} the prompt was sent and the session runs without these options"
    );
    assert_eq!(options.telemetry_message(), Some("option_not_applied"));
    assert_eq!(ClaimError::AlreadyClaimed.claim(), None);
}

#[test]
fn claim_errors_without_a_telemetry_message_or_with_a_local_cause() {
    assert_eq!(ClaimError::AlreadyClaimed.telemetry_message(), None);
    assert_eq!(ClaimError::SpareExited.telemetry_message(), None);
    let refused = ClaimError::Refused(Arc::new(Error::TooManyControls));
    assert_eq!(refused.telemetry_message(), None);
    let local = ClaimError::Local(Arc::new(Error::TooManyControls));
    assert_eq!(local.to_string(), "too many outstanding control requests");
    assert_eq!(local.telemetry_message(), None);
}

/// A filesystem whose `create_directory` answers from a script and records every path it was asked for.
#[cfg(unix)]
#[derive(Default)]
struct ScriptedDirectories {
    outcomes: Mutex<std::collections::VecDeque<Option<std::io::ErrorKind>>>,
    created: Mutex<Vec<PathBuf>>,
}

#[cfg(unix)]
impl ScriptedDirectories {
    fn new(outcomes: impl IntoIterator<Item = Option<std::io::ErrorKind>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            created: Mutex::new(Vec::new()),
        }
    }

    fn created(&self) -> Vec<PathBuf> {
        locked(&self.created).clone()
    }
}

#[cfg(unix)]
impl crate::adapters::CreateDirectory for ScriptedDirectories {
    type Error = std::io::Error;

    fn create_directory(&self, _builder: &std::fs::DirBuilder, path: &Path) -> std::io::Result<()> {
        locked(&self.created).push(path.to_path_buf());
        match locked(&self.outcomes).pop_front().flatten() {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }
}

#[cfg(unix)]
impl crate::adapters::CanonicalizePath for ScriptedDirectories {
    type Error = std::io::Error;

    fn canonicalize(&self, _path: &Path) -> std::io::Result<PathBuf> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
impl crate::adapters::OpenFile for ScriptedDirectories {
    type Error = std::io::Error;

    fn open(&self, _options: &std::fs::OpenOptions, _path: &Path) -> std::io::Result<std::fs::File> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
impl crate::adapters::ReadMetadata for ScriptedDirectories {
    type Error = std::io::Error;

    fn metadata(&self, _path: &Path) -> std::io::Result<std::fs::Metadata> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
impl crate::adapters::RemoveDirectoryAll for ScriptedDirectories {
    type Error = std::io::Error;

    fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
impl crate::adapters::RemoveFile for ScriptedDirectories {
    type Error = std::io::Error;

    fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
fn parent_environment(variables: &[(&str, &str)]) -> crate::test_support::FixedEnvironment {
    crate::test_support::FixedEnvironment {
        directory: PathBuf::from("/work"),
        variables: variables
            .iter()
            .map(|&(key, value)| (std::ffi::OsString::from(key), std::ffi::OsString::from(value)))
            .collect(),
    }
}

/// Options whose `env` replaces the parent environment, as TypeScript does for a non-empty `options.env`.
#[cfg(unix)]
fn replacing_options(env: &[(&str, &str)]) -> ClaudeAgentOptions {
    let mut options = ClaudeAgentOptions::default();
    for &(key, value) in env {
        options.env.insert(key.to_owned(), value.into());
    }
    options
}

#[cfg(unix)]
fn assert_parked_under(path: &Path, spares: &str) {
    assert_eq!(path.parent(), Some(Path::new(spares)));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert_eq!(name.len(), "spare-".len() + 6, "{name}");
    assert!(name.starts_with("spare-"), "{name}");
}

#[cfg(unix)]
#[test]
fn a_replacing_environment_parks_under_its_own_home() -> Result<(), Error> {
    let filesystem = ScriptedDirectories::default();
    let path = park_directory(
        &replacing_options(&[("HOME", "/h")]),
        &parent_environment(&[("HOME", "/p"), ("CLAUDE_CONFIG_DIR", "/ignored")]),
        &filesystem,
    )?;
    assert_parked_under(&path, "/h/.claude/spares");
    assert_eq!(filesystem.created(), vec![PathBuf::from("/h/.claude/spares"), path]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_replacing_environment_without_a_home_parks_under_the_process_home() -> Result<(), Error> {
    let filesystem = ScriptedDirectories::default();
    let path = park_directory(
        &replacing_options(&[("K", "v")]),
        &parent_environment(&[("HOME", "/p")]),
        &filesystem,
    )?;
    assert_parked_under(&path, "/p/.claude/spares");
    Ok(())
}

#[cfg(unix)]
#[test]
fn no_absolute_config_home_is_an_option_error() {
    let filesystem = ScriptedDirectories::default();
    let result = park_directory(
        &replacing_options(&[("CLAUDE_CONFIG_DIR", "relative")]),
        &parent_environment(&[]),
        &filesystem,
    );
    assert!(
        matches!(&result, Err(Error::Option(error)) if error.message() == "prewarm(): no absolute config home to park the spare under (neither CLAUDE_CONFIG_DIR nor HOME gives an absolute path); set one of them to an absolute path or pass options.cwd"),
        "{result:?}"
    );
    assert!(filesystem.created().is_empty());
}

#[cfg(unix)]
#[test]
fn a_taken_spare_name_is_drawn_again() -> Result<(), Error> {
    let filesystem = ScriptedDirectories::new([None, Some(std::io::ErrorKind::AlreadyExists), None]);
    let path = park_directory(
        &replacing_options(&[("CLAUDE_CONFIG_DIR", "/c")]),
        &parent_environment(&[]),
        &filesystem,
    )?;
    assert_parked_under(&path, "/c/spares");
    assert_eq!(filesystem.created().len(), 3);
    Ok(())
}

#[cfg(unix)]
#[test]
fn spare_names_taken_nine_times_in_a_row_give_up() {
    let outcomes = std::iter::once(None).chain(std::iter::repeat_n(Some(std::io::ErrorKind::AlreadyExists), 9));
    let filesystem = ScriptedDirectories::new(outcomes);
    let result = park_directory(
        &replacing_options(&[("CLAUDE_CONFIG_DIR", "/c")]),
        &parent_environment(&[]),
        &filesystem,
    );
    assert!(
        matches!(&result, Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists),
        "{result:?}"
    );
    assert_eq!(filesystem.created().len(), 10);
}

#[cfg(unix)]
#[test]
fn a_spare_directory_that_cannot_be_created_is_an_io_error() {
    let filesystem = ScriptedDirectories::new([None, Some(std::io::ErrorKind::PermissionDenied)]);
    let result = park_directory(
        &replacing_options(&[("CLAUDE_CONFIG_DIR", "/c")]),
        &parent_environment(&[]),
        &filesystem,
    );
    assert!(
        matches!(&result, Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied),
        "{result:?}"
    );
    assert_eq!(filesystem.created().len(), 2);
}
