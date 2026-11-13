#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Upstream-shaped fixtures are written as JSON literals.")
)]

use serde_json::{Value, json};
use trogon_claude_agent_sdk::upstream::settings::{
    SettingsExtraKnownMarketplacesValueSource as MarketplaceSource, SettingsHooksValueItemHooksItem as HookCommand,
};
use trogon_claude_agent_sdk::{
    PolicySettingsOrigin, ProvenanceEntry, ResolveSettingsOptions, ResolvedSettingSource, ResolvedSettings,
    SDKSettingsParseError, SandboxIgnoreViolations, SandboxNetworkConfig, SandboxSettings, Settings,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn round_trips(value: &Value) -> Result<Settings, Box<dyn std::error::Error>> {
    let text = serde_json::to_string(value)?;
    let typed = Settings::from_json(&text)?;
    assert_eq!(typed.to_json()?, text);
    Ok(typed)
}

#[test]
fn each_hook_handler_kind_decodes_into_its_own_variant() -> TestResult {
    let typed = round_trips(&json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
        {"type": "command", "command": "echo", "shell": "bash", "timeout": 5, "onFailure": "block"},
        {"type": "prompt", "prompt": "check it", "model": "m"},
        {"type": "agent", "prompt": "verify"},
        {"type": "http", "url": "https://hooks.example/x", "headers": {"A": "b"}, "allowedEnvVars": ["TOKEN"]},
        {"type": "mcp_tool", "server": "s", "tool": "t", "input": {"k": 1}},
        {"type": "future", "x": 1}
    ]}]}}))?;
    let hooks = typed.hooks.ok_or("hooks missing")?;
    let entries = hooks.get("PreToolUse").ok_or("PreToolUse missing")?;
    let kinds: Vec<&'static str> = entries
        .iter()
        .flat_map(|entry| entry.hooks.iter())
        .map(|hook| match hook {
            HookCommand::Command(_) => "command",
            HookCommand::Prompt(_) => "prompt",
            HookCommand::Agent(_) => "agent",
            HookCommand::Http(_) => "http",
            HookCommand::McpTool(_) => "mcp_tool",
            HookCommand::Unknown(_) => "unknown",
        })
        .collect();
    assert_eq!(kinds, ["command", "prompt", "agent", "http", "mcp_tool", "unknown"]);
    Ok(())
}

#[test]
fn each_marketplace_source_kind_decodes_into_its_own_variant() -> TestResult {
    let sources = [
        (
            "url",
            json!({"source": "url", "url": "https://m.example/m.json", "headers": {"A": "b"}}),
        ),
        (
            "github",
            json!({"source": "github", "repo": "owner/repo", "sparsePaths": ["a"]}),
        ),
        (
            "git",
            json!({"source": "git", "url": "https://h.example/r.git", "path": "p"}),
        ),
        ("npm", json!({"source": "npm", "package": "pkg", "version": "1"})),
        ("file", json!({"source": "file", "path": "/m.json"})),
        ("directory", json!({"source": "directory", "path": "/dir"})),
        ("skillsDir", json!({"source": "skillsDir"})),
        ("hostPattern", json!({"source": "hostPattern", "hostPattern": "^h"})),
        ("pathPattern", json!({"source": "pathPattern", "pathPattern": "^/p"})),
        (
            "settings",
            json!({"source": "settings", "name": "inline", "owner": {"name": "o"}, "plugins": [
                {"name": "a", "source": {"source": "github", "repo": "o/r"}},
                {"name": "b", "source": {"source": "archive", "url": "https://x/a.tgz", "sha256": "ab"}}
            ]}),
        ),
    ];
    for (kind, source) in sources {
        let typed = round_trips(&json!({"extraKnownMarketplaces": {"m": {"source": source, "autoUpdate": true}}}))?;
        let markets = typed.extra_known_marketplaces.ok_or("marketplaces missing")?;
        let decoded = &markets.get("m").ok_or("m missing")?.source;
        let name = match decoded {
            MarketplaceSource::Url(_) => "url",
            MarketplaceSource::Github(_) => "github",
            MarketplaceSource::Git(_) => "git",
            MarketplaceSource::Npm(_) => "npm",
            MarketplaceSource::File(_) => "file",
            MarketplaceSource::Directory(_) => "directory",
            MarketplaceSource::SkillsDir(_) => "skillsDir",
            MarketplaceSource::HostPattern(_) => "hostPattern",
            MarketplaceSource::PathPattern(_) => "pathPattern",
            MarketplaceSource::Settings(_) => "settings",
            MarketplaceSource::Unknown(_) => "unknown",
        };
        assert_eq!(name, kind, "marketplace source classified wrongly");
    }
    Ok(())
}

#[test]
fn an_unlisted_marketplace_source_survives_as_raw_json() -> TestResult {
    let typed = round_trips(&json!({"extraKnownMarketplaces": {"m": {"source": {"source": "ftp", "host": "h"}}}}))?;
    let markets = typed.extra_known_marketplaces.ok_or("marketplaces missing")?;
    let decoded = &markets.get("m").ok_or("m missing")?.source;
    assert!(
        !matches!(decoded, MarketplaceSource::Url(_) | MarketplaceSource::Git(_)),
        "{decoded:?}"
    );
    Ok(())
}

#[test]
fn status_line_ssh_configs_and_plugin_configs_round_trip_with_unknown_members() -> TestResult {
    let typed = round_trips(&json!({
        "statusLine": {"type": "command", "command": "~/s.sh", "padding": 1, "refreshIntervalMs": 0, "next": null},
        "sshConfigs": [
            {"id": "a", "name": "A", "sshHost": "u@h", "sshPort": 2222, "sshIdentityFile": "~/.ssh/k", "startDirectory": "~/p"},
            {"id": "b", "name": "B", "sshHost": "h", "later": [1]}
        ],
        "enabledPlugins": {"a@m": true, "b@m": ["x"], "c@m": {"future": 1}},
        "pluginConfigs": {"a@m": {"mcpServers": {"s": {"k": "v"}}, "options": {"o": 1}}, "b@m": {"free": "form"}}
    }))?;
    assert_eq!(typed.ssh_configs.as_ref().map(Vec::len), Some(2));
    assert!(typed.status_line.is_some());
    Ok(())
}

#[test]
fn python_sandbox_dicts_map_byte_for_byte_onto_the_typed_sandbox() -> TestResult {
    // Python `SandboxSettings` / `SandboxNetworkConfig` TypedDicts, with the keys in the order a
    // Python caller would write them (json.dumps default separators removed).
    let fixtures = [
        r#"{"enabled":true,"autoAllowBashIfSandboxed":false,"excludedCommands":["git","docker"],"allowUnsandboxedCommands":true,"network":{"allowedDomains":["a.example"],"deniedDomains":["b.example"],"allowManagedDomainsOnly":true,"allowUnixSockets":["/s"],"allowAllUnixSockets":false,"allowLocalBinding":true,"allowMachLookup":["m*"],"httpProxyPort":8080,"socksProxyPort":1080},"ignoreViolations":{"*":["x"],"git":["y","z"]},"enableWeakerNestedSandbox":false}"#,
        r#"{"enabled":false}"#,
        r#"{"network":{"httpProxyPort":8080}}"#,
        r#"{}"#,
    ];
    for text in fixtures {
        let typed: SandboxSettings = serde_json::from_str(text)?;
        assert_eq!(serde_json::to_string(&typed)?, text);
        assert!(typed.extra.is_empty(), "a Python key was not typed: {text}");
        let wrapped = format!(r#"{{"sandbox":{text}}}"#);
        assert_eq!(Settings::from_json(&wrapped)?.to_json()?, wrapped);
    }
    let network: SandboxNetworkConfig = serde_json::from_str(r#"{"allowedDomains":["a"],"httpProxyPort":1}"#)?;
    assert_eq!(
        network.http_proxy_port.as_ref().map(ToString::to_string),
        Some("1".to_owned())
    );
    let ignore: SandboxIgnoreViolations = serde_json::from_str(r#"{"*":["a"]}"#)?;
    assert_eq!(ignore.len(), 1);
    Ok(())
}

#[test]
fn typescript_only_sandbox_keys_and_unknown_keys_are_kept() -> TestResult {
    let text = r#"{"failIfUnavailable":true,"filesystem":{"allowWrite":["/w"],"denyRead":["/r"]},"credentials":{"allowPlaintextInject":false,"sigv4":{"streaming":"deny"}},"ripgrep":{"command":"rg","args":["-n"]},"bwrapPath":"/bwrap","socatPath":"/socat","futureKnob":{"a":1}}"#;
    let typed: SandboxSettings = serde_json::from_str(text)?;
    assert_eq!(
        typed.extra.keys().map(String::as_str).collect::<Vec<_>>(),
        ["futureKnob"]
    );
    assert_eq!(serde_json::to_string(&typed)?, text);
    Ok(())
}

#[test]
fn resolved_settings_round_trip_with_provenance_and_machine_policy() -> TestResult {
    let text = r#"{"effective":{"model":"opus","futureKey":1},"provenance":{"model":{"source":"managed","policyOrigin":"file","path":"/etc/m.json"},"futureKey":{"source":"flag"}},"sources":[{"source":"user","settings":{"model":"sonnet"},"path":"/u/settings.json"},{"source":"managed","settings":{"model":"opus"},"policyOrigin":"hklm"}],"machinePolicy":{"didNotLoad":["profile","registry"]}}"#;
    let typed: ResolvedSettings = serde_json::from_str(text)?;
    assert_eq!(typed.sources.len(), 2);
    assert_eq!(typed.effective.model.as_deref(), Some("opus"));
    assert_eq!(serde_json::to_string(&typed)?, text);

    let none = r#"{"effective":{},"provenance":{},"sources":[],"machinePolicy":{"didNotLoad":false}}"#;
    assert_eq!(
        serde_json::to_string(&serde_json::from_str::<ResolvedSettings>(none)?)?,
        none
    );

    let absent = r#"{"effective":{},"provenance":{},"sources":[]}"#;
    assert_eq!(
        serde_json::to_string(&serde_json::from_str::<ResolvedSettings>(absent)?)?,
        absent
    );
    Ok(())
}

#[test]
fn every_policy_origin_and_source_name_decodes_and_unknown_ones_are_kept() -> TestResult {
    for origin in ["helper", "remote", "plist", "hklm", "file", "parent", "hkcu"] {
        let typed: PolicySettingsOrigin = serde_json::from_value(json!(origin))?;
        assert!(!matches!(typed, PolicySettingsOrigin::Unknown(_)), "{origin}");
        assert_eq!(serde_json::to_value(&typed)?, json!(origin));
    }
    for source in ["user", "project", "local", "managed", "flag", "policy"] {
        let typed: ResolvedSettingSource = serde_json::from_value(json!(source))?;
        assert_eq!(serde_json::to_value(&typed)?, json!(source));
    }
    let entry: ProvenanceEntry = serde_json::from_value(json!({"source": "managed", "policyOrigin": "plist"}))?;
    assert_eq!(entry.policy_origin, Some(PolicySettingsOrigin::Plist));
    Ok(())
}

#[test]
fn resolve_options_and_parse_errors_round_trip() -> TestResult {
    let text = r#"{"cwd":"/p","settingSources":["user","local"],"managedSettings":{"permissions":{"deny":["Bash"]}},"serverManagedSettings":{"model":"m"}}"#;
    let options: ResolveSettingsOptions = serde_json::from_str(text)?;
    assert_eq!(options.cwd.as_deref(), Some("/p"));
    assert_eq!(serde_json::to_string(&options)?, text);

    let empty: ResolveSettingsOptions = serde_json::from_str(r#"{"settingSources":[]}"#)?;
    assert_eq!(serde_json::to_string(&empty)?, r#"{"settingSources":[]}"#);

    let whole_file = r#"{"path":"","message":"Unexpected token"}"#;
    let error: SDKSettingsParseError = serde_json::from_str(whole_file)?;
    assert_eq!(error.file, None);
    assert_eq!(serde_json::to_string(&error)?, whole_file);

    let located = r#"{"file":"/p/.claude/settings.json","path":"permissions.deny","message":"bad"}"#;
    assert_eq!(
        serde_json::to_string(&serde_json::from_str::<SDKSettingsParseError>(located)?)?,
        located
    );
    Ok(())
}
