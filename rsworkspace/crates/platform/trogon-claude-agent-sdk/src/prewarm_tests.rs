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
