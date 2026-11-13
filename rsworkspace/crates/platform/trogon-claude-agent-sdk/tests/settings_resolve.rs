#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    PythonJsonValue, ResolveSettingsOptions, ResolvedSettings, Settings, SettingsDialect, adapters::OperatingSystem,
    filter_escalating_default_mode, js_stringify, python_json_dumps, resolve_python_settings_with,
    resolve_settings_in_dialect_with, resolve_settings_with,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::{FakeHost, exited, ok, spawn_error};

fn options(value: Value) -> ResolveSettingsOptions {
    serde_json::from_value(value).expect("options")
}

fn resolve(host: &FakeHost, value: Value) -> Value {
    let resolved = resolve_settings_with(host, &options(value)).expect("resolve");
    serde_json::to_value(resolved).expect("encode")
}

fn cascade_host() -> FakeHost {
    FakeHost::linux()
        .file(
            "/home/fixture/.claude/settings.json",
            r#"{"model":"user-model","permissions":{"allow":["Bash(a)"]},"env":{"A":"1"}}"#,
        )
        .file(
            "/work/project/.claude/settings.json",
            r#"{"model":"project-model","permissions":{"allow":["Bash(b)","Bash(a)"]}}"#,
        )
        .file("/work/project/.claude/settings.local.json", r#"{"env":{"B":"2"}}"#)
}

#[test]
fn cascade_merges_in_precedence_order_with_array_union_and_recursion() {
    let result = resolve(&cascade_host(), json!({}));
    assert_eq!(
        result["effective"],
        json!({
            "model": "project-model",
            "permissions": {"allow": ["Bash(a)", "Bash(b)"]},
            "env": {"A": "1", "B": "2"}
        })
    );
    assert_eq!(
        result["provenance"],
        json!({
            "model": {"source": "project", "path": "/work/project/.claude/settings.json"},
            "permissions": {"source": "project", "path": "/work/project/.claude/settings.json"},
            "env": {"source": "local", "path": "/work/project/.claude/settings.local.json"}
        })
    );
    assert_eq!(result["machinePolicy"], json!({"didNotLoad": false}));
}

#[test]
fn sources_list_each_non_empty_source_low_to_high() {
    let result = resolve(&cascade_host(), json!({}));
    let sources = result["sources"].as_array().expect("sources");
    let labels: Vec<_> = sources.iter().map(|item| item["source"].as_str().unwrap()).collect();
    assert_eq!(labels, ["user", "project", "local"]);
    assert_eq!(sources[0]["path"], "/home/fixture/.claude/settings.json");
    assert_eq!(sources[0]["settings"]["model"], "user-model");
}

#[test]
fn empty_setting_sources_skip_every_filesystem_source() {
    let result = resolve(&cascade_host(), json!({"settingSources": []}));
    assert_eq!(result["effective"], json!({}));
    assert_eq!(result["sources"], json!([]));
}

#[test]
fn explicit_cwd_selects_the_project_files() {
    let host = cascade_host().file("/elsewhere/.claude/settings.json", r#"{"model":"elsewhere"}"#);
    let result = resolve(&host, json!({"cwd": "/elsewhere", "settingSources": ["project"]}));
    assert_eq!(result["effective"], json!({"model": "elsewhere"}));
}

#[test]
fn config_dir_variable_moves_the_user_file() {
    let host = FakeHost::linux()
        .variable("CLAUDE_CONFIG_DIR", "/cfg")
        .file("/cfg/settings.json", r#"{"model":"configured"}"#);
    let result = resolve(&host, json!({"settingSources": ["user"]}));
    assert_eq!(result["provenance"]["model"]["path"], "/cfg/settings.json");
}

#[test]
fn invalid_and_non_object_files_are_skipped_without_failing() {
    let host = FakeHost::linux()
        .file("/home/fixture/.claude/settings.json", "{ not json")
        .file("/work/project/.claude/settings.json", "[1]")
        .file("/work/project/.claude/settings.local.json", "   \n");
    let result = resolve(&host, json!({}));
    assert_eq!(result["effective"], json!({}));
    assert_eq!(result["sources"], json!([]));
}

#[test]
fn managed_file_wins_and_reports_its_origin() {
    let host = cascade_host().file("/etc/claude-code/managed-settings.json", r#"{"model":"managed-model"}"#);
    let result = resolve(&host, json!({}));
    assert_eq!(result["effective"]["model"], "managed-model");
    assert_eq!(
        result["provenance"]["model"],
        json!({"source": "managed", "policyOrigin": "file"})
    );
    let last = result["sources"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        last,
        json!({"source": "managed", "settings": {"model": "managed-model"}, "policyOrigin": "file"})
    );
}

#[test]
fn managed_drop_ins_merge_in_code_unit_order_after_the_base_file() {
    let host = FakeHost::linux()
        .file(
            "/etc/claude-code/managed-settings.json",
            r#"{"env":{"BASE":"1"},"model":"base"}"#,
        )
        .directory(
            "/etc/claude-code/managed-settings.d",
            &["20-b.json", "10-a.json", ".hidden.json", "notes.txt"],
        )
        .file(
            "/etc/claude-code/managed-settings.d/10-a.json",
            r#"{"model":"a","env":{"A":"1"}}"#,
        )
        .file("/etc/claude-code/managed-settings.d/20-b.json", r#"{"model":"b"}"#)
        .file(
            "/etc/claude-code/managed-settings.d/.hidden.json",
            r#"{"model":"hidden"}"#,
        );
    let result = resolve(&host, json!({"settingSources": []}));
    assert_eq!(
        result["effective"],
        json!({"env": {"BASE": "1", "A": "1"}, "model": "b"})
    );
}

#[test]
fn unreadable_managed_file_reports_did_not_load() {
    let host = FakeHost::linux().file("/etc/claude-code/managed-settings.json", "{ broken");
    let result = resolve(&host, json!({"settingSources": []}));
    assert_eq!(result["machinePolicy"], json!({"didNotLoad": ["file"]}));
}

#[test]
fn remote_policy_outranks_the_managed_file() {
    let host = FakeHost::linux().file("/etc/claude-code/managed-settings.json", r#"{"model":"file"}"#);
    let result = resolve(
        &host,
        json!({"settingSources": [], "serverManagedSettings": {"model": "remote", "cleanupPeriodDays": 3}}),
    );
    assert_eq!(result["effective"]["model"], "remote");
    assert_eq!(result["provenance"]["model"]["policyOrigin"], "remote");
}

#[test]
fn parent_managed_settings_contribute_only_restrictive_keys() {
    let result = resolve(
        &FakeHost::linux(),
        json!({
            "settingSources": [],
            "managedSettings": {
                "model": "ignored",
                "allowManagedHooksOnly": true,
                "permissions": {"deny": ["Bash(rm:*)", "Read(!secret)"], "allow": ["Bash(ls)"]}
            }
        }),
    );
    assert_eq!(result["effective"]["allowManagedHooksOnly"], true);
    assert_eq!(result["effective"].get("model"), None);
    assert_eq!(result["effective"]["permissions"]["deny"], json!(["Bash(rm:*)"]));
    assert_eq!(result["provenance"]["allowManagedHooksOnly"]["policyOrigin"], "parent");
}

#[test]
fn plist_policy_is_read_with_plutil_on_macos() {
    let mut host = FakeHost::linux();
    host.system = OperatingSystem::Darwin;
    let plist = "/Library/Managed Preferences/fixture-user/com.anthropic.claudecode.plist";
    let device = "/Library/Managed Preferences/com.anthropic.claudecode.plist";
    let host = host.file(plist, "bplist").file(device, "bplist").command(
        &format!("/usr/bin/plutil -convert json -o - -- {plist}"),
        ok(r#"{"model":"plist-model"}"#),
    );
    let result = resolve(&host, json!({"settingSources": []}));
    assert_eq!(result["effective"], json!({"model": "plist-model"}));
    assert_eq!(result["provenance"]["model"]["policyOrigin"], "plist");
    assert!(
        host.ran()
            .contains(&format!("/usr/bin/plutil -convert json -o - -- {plist}"))
    );
}

#[test]
fn a_plist_plutil_cannot_convert_is_linted_and_read_as_empty_when_the_lint_fails() {
    let mut host = FakeHost::linux();
    host.system = OperatingSystem::Darwin;
    let plist = "/Library/Managed Preferences/fixture-user/com.anthropic.claudecode.plist";
    let device = "/Library/Managed Preferences/com.anthropic.claudecode.plist";
    let lint = format!("/usr/bin/plutil -lint -s -- {plist}");
    let host = host
        .file(plist, "bplist")
        .file(device, "bplist")
        .command(
            &format!("/usr/bin/plutil -convert json -o - -- {plist}"),
            exited(1, "invalid"),
        )
        .command(&lint, exited(1, "invalid"));
    let result = resolve(&host, json!({"settingSources": []}));
    assert_eq!(result["effective"].get("model"), None);
    assert!(host.ran().contains(&lint));
}

/// What the pinned TypeScript resolver printed, recorded from its oracle on darwin, for a per-user managed preferences
/// plist that does not load while the device-level one is absent: the plist cannot be read, or plutil cannot convert
/// it but lints it clean.
const PLIST_DID_NOT_LOAD: &str = r#"{"result":{"effective":{},"provenance":{},"sources":[],"machinePolicy":{"didNotLoad":["profile"]}},"filtered":{}}"#;

/// The result and its filtered settings, printed the way the TypeScript resolver's oracle prints them.
fn printed(host: &FakeHost, value: Value) -> String {
    let resolved = resolve_settings_with(host, &options(value)).expect("resolve");
    js_stringify(&json!({
        "result": serde_json::to_value(&resolved).expect("encode"),
        "filtered": serde_json::to_value(filter_escalating_default_mode(&resolved)).expect("encode"),
    }))
}

#[test]
fn a_plist_that_cannot_be_read_does_not_load_and_an_absent_one_is_skipped_without_plutil() {
    let mut host = FakeHost::linux();
    host.system = OperatingSystem::Darwin;
    let plist = "/Library/Managed Preferences/fixture-user/com.anthropic.claudecode.plist";
    host.unreadable.insert(plist.into());
    let host = host.file(plist, "bplist");
    assert_eq!(printed(&host, json!({"settingSources": []})), PLIST_DID_NOT_LOAD);
    assert!(!host.ran().iter().any(|line| line.starts_with("/usr/bin/plutil")));
}

#[test]
fn a_plist_plutil_cannot_convert_but_lints_clean_does_not_load() {
    let mut host = FakeHost::linux();
    host.system = OperatingSystem::Darwin;
    let plist = "/Library/Managed Preferences/fixture-user/com.anthropic.claudecode.plist";
    let lint = format!("/usr/bin/plutil -lint -s -- {plist}");
    let host = host
        .file(plist, "bplist")
        .command(
            &format!("/usr/bin/plutil -convert json -o - -- {plist}"),
            exited(1, "invalid"),
        )
        .command(&lint, ok(""));
    assert_eq!(printed(&host, json!({"settingSources": []})), PLIST_DID_NOT_LOAD);
    assert!(host.ran().contains(&lint));
}

#[test]
fn registry_policy_is_read_with_reg_on_windows() {
    let mut host = FakeHost::linux();
    host.system = OperatingSystem::Win32;
    let machine = "C:\\Windows\\System32\\reg.exe query HKLM\\SOFTWARE\\Policies\\ClaudeCode /v Settings";
    let host = host.command(
        machine,
        ok("\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\ClaudeCode\r\n    Settings    REG_SZ    {\"model\":\"hklm-model\"}\r\n"),
    );
    let result = resolve(&host, json!({"settingSources": []}));
    assert_eq!(result["effective"], json!({"model": "hklm-model"}));
    assert_eq!(result["provenance"]["model"]["policyOrigin"], "hklm");
    assert_eq!(result["machinePolicy"], json!({"didNotLoad": false}));
}

#[test]
fn a_busy_registry_read_is_reported_but_a_missing_key_is_not() {
    let mut busy = FakeHost::linux();
    busy.system = OperatingSystem::Win32;
    let line = "C:\\Windows\\System32\\reg.exe query HKLM\\SOFTWARE\\Policies\\ClaudeCode /v Settings";
    let busy = busy.command(line, spawn_error("EAGAIN"));
    assert_eq!(
        resolve(&busy, json!({"settingSources": []}))["machinePolicy"],
        json!({"didNotLoad": ["registry"]})
    );
    let mut missing = FakeHost::linux();
    missing.system = OperatingSystem::Win32;
    let missing = missing.command(
        line,
        exited(
            1,
            "ERROR: The system was unable to find the specified registry key or value.",
        ),
    );
    assert_eq!(
        resolve(&missing, json!({"settingSources": []}))["machinePolicy"],
        json!({"didNotLoad": false})
    );
}

#[test]
fn wsl_without_the_inherit_flag_ignores_the_windows_side() {
    let host = FakeHost::linux()
        .variable("WSL_DISTRO_NAME", "Ubuntu")
        .file(
            "/mnt/c/Program Files/ClaudeCode/managed-settings.json",
            r#"{"model":"windows"}"#,
        )
        .file("/etc/claude-code/managed-settings.json", r#"{"model":"linux"}"#);
    let result = resolve(&host, json!({"settingSources": []}));
    assert_eq!(result["effective"]["model"], "linux");
}

#[test]
fn wsl_with_the_inherit_flag_reads_the_windows_side_first() {
    let host = FakeHost::linux()
        .variable("WSL_DISTRO_NAME", "Ubuntu")
        .file(
            "/mnt/c/Program Files/ClaudeCode/managed-settings.json",
            r#"{"wslInheritsWindowsSettings":true,"model":"windows"}"#,
        )
        .file("/etc/claude-code/managed-settings.json", r#"{"model":"linux"}"#);
    let result = resolve(&host, json!({"settingSources": []}));
    assert_eq!(result["effective"]["model"], "windows");
}

#[test]
fn wsl_withholds_the_linux_policy_when_the_windows_side_file_is_not_an_object() {
    // Recorded from the pinned TypeScript resolver with the same two files.
    let host = FakeHost::linux()
        .variable("WSL_DISTRO_NAME", "Ubuntu")
        .file("/mnt/c/Program Files/ClaudeCode/managed-settings.json", "[1]")
        .file("/etc/claude-code/managed-settings.json", r#"{"model":"linux"}"#);
    let result = resolve(&host, json!({"settingSources": []}));
    assert_eq!(
        js_stringify(&result),
        r#"{"effective":{},"provenance":{},"sources":[],"machinePolicy":{"didNotLoad":["registry"]}}"#
    );
}

fn resolved(effective: Value, sources: Value) -> ResolvedSettings {
    serde_json::from_value(json!({
        "effective": effective,
        "provenance": {},
        "sources": sources,
        "machinePolicy": {"didNotLoad": false}
    }))
    .expect("resolved")
}

fn filtered(resolved: &ResolvedSettings) -> Value {
    let settings: Settings = filter_escalating_default_mode(resolved);
    serde_json::to_value(settings).expect("encode")
}

#[test]
fn escalating_default_mode_from_a_project_source_is_dropped() {
    let resolved = resolved(
        json!({"permissions": {"defaultMode": "bypassPermissions", "allow": ["x"]}}),
        json!([{"source": "project", "settings": {"permissions": {"defaultMode": "bypassPermissions"}}}]),
    );
    assert_eq!(filtered(&resolved), json!({"permissions": {"allow": ["x"]}}));
}

#[test]
fn accept_edits_from_a_local_source_survives_but_not_from_a_project_source() {
    let local = resolved(
        json!({"permissions": {"defaultMode": "acceptEdits"}}),
        json!([{"source": "local", "settings": {"permissions": {"defaultMode": "acceptEdits"}}}]),
    );
    assert_eq!(filtered(&local), json!({"permissions": {"defaultMode": "acceptEdits"}}));
    let project = resolved(
        json!({"permissions": {"defaultMode": "acceptEdits"}}),
        json!([{"source": "project", "settings": {"permissions": {"defaultMode": "acceptEdits"}}}]),
    );
    assert_eq!(filtered(&project), json!({"permissions": {}}));
}

#[test]
fn default_mode_set_by_a_user_source_is_kept() {
    let resolved = resolved(
        json!({"permissions": {"defaultMode": "auto"}}),
        json!([
            {"source": "user", "settings": {"permissions": {"defaultMode": "auto"}}}
        ]),
    );
    assert_eq!(filtered(&resolved), json!({"permissions": {"defaultMode": "auto"}}));
}

fn effective_in(dialect: SettingsDialect, text: &str) -> Value {
    let host = FakeHost::linux().file("/work/project/.claude/settings.json", text);
    let resolved = resolve_settings_in_dialect_with(&host, &options(json!({})), dialect).expect("resolve");
    serde_json::to_value(&resolved.effective).expect("encode")
}

fn custom_numbers(dialect: SettingsDialect) -> String {
    let text = r#"{"custom":{"n":12345678901234567890,"m":9007199254740993,"z":-0,"w":1.0,"f":0.1}}"#;
    let effective = effective_in(dialect, text);
    match dialect {
        SettingsDialect::TypeScript => js_stringify(&effective),
        _ => effective.to_string(),
    }
}

#[test]
fn js_stringify_prints_resolved_numbers_as_the_typescript_resolver_does() {
    let text = r#"{"custom":{"a":18446744073709551616,"b":-9223372036854775809,"c":0.0000015,"d":1e-7,"e":1e21}}"#;
    let effective = effective_in(SettingsDialect::TypeScript, text);
    assert_eq!(
        js_stringify(&effective),
        r#"{"custom":{"a":18446744073709552000,"b":-9223372036854776000,"c":0.0000015,"d":1e-7,"e":1e+21}}"#
    );
}

#[test]
fn the_typescript_dialect_reads_numbers_like_json_parse() {
    assert_eq!(
        custom_numbers(SettingsDialect::TypeScript),
        r#"{"custom":{"n":12345678901234567000,"m":9007199254740992,"z":0,"w":1,"f":0.1}}"#
    );
    assert_eq!(
        custom_numbers(SettingsDialect::default()),
        custom_numbers(SettingsDialect::TypeScript)
    );
}

#[test]
fn the_python_dialect_keeps_integer_tokens_exact() {
    assert_eq!(
        custom_numbers(SettingsDialect::Python),
        r#"{"custom":{"n":12345678901234567890,"m":9007199254740993,"z":0,"w":1.0,"f":0.1}}"#
    );
}

#[test]
fn the_python_resolve_keeps_every_number_as_json_loads_reads_it() {
    let host = FakeHost::linux()
        .file(
            "/home/fixture/.claude/settings.json",
            r#"{"custom":{"big":[123456789012345678901234567890,1.0]},"env":{"A":123456789012345678901234567890}}"#,
        )
        .file(
            "/work/project/.claude/settings.json",
            r#"{"custom":{"big":[123456789012345678901234567890,-98765432109876543210987654321,1.0,NaN,Infinity,1e400],"w":2.0,"z":-0.0}}"#,
        );
    let resolved = resolve_python_settings_with(&host, &options(json!({}))).expect("resolve");
    let PythonJsonValue::Object(resolved) = resolved else {
        panic!("the resolved settings are not an object");
    };
    assert_eq!(
        python_json_dumps(resolved.get("effective").expect("effective")),
        r#"{"env": {"A": "1.2345678901234568e+29"}, "custom": {"big": [123456789012345678901234567890, 1.0, -98765432109876543210987654321, NaN, Infinity], "w": 2.0, "z": -0.0}}"#
    );
    let approximate = effective_in(
        SettingsDialect::Python,
        r#"{"custom":{"big":[123456789012345678901234567890,1.0,NaN,Infinity],"w":2.0}}"#,
    );
    assert_eq!(
        approximate.to_string(),
        r#"{"custom":{"big":[1.2345678901234568e+29,1.0,null,null],"w":2.0}}"#
    );
}

#[test]
fn the_python_resolve_unions_arrays_by_the_number_javascript_reads() {
    let host = FakeHost::linux()
        .file("/home/fixture/.claude/settings.json", r#"{"custom":{"l":[1,0]}}"#)
        .file(
            "/work/project/.claude/settings.json",
            r#"{"custom":{"l":[1.0,-0.0,2,9223372036854775808.0]}}"#,
        );
    let resolved = resolve_python_settings_with(&host, &options(json!({}))).expect("resolve");
    let PythonJsonValue::Object(resolved) = resolved else {
        panic!("the resolved settings are not an object");
    };
    assert_eq!(
        python_json_dumps(resolved.get("effective").expect("effective")),
        r#"{"custom": {"l": [1, 0, 2, 9.223372036854776e+18]}}"#
    );
}

#[test]
fn the_python_dialect_keeps_a_whole_float_of_two_to_the_sixty_third_finite() {
    let effective = effective_in(SettingsDialect::Python, r#"{"custom":{"w":9223372036854775808.0}}"#);
    assert_eq!(effective.to_string(), r#"{"custom":{"w":9.223372036854776e+18}}"#);
}

fn effective_text(host: &FakeHost) -> (String, Vec<String>) {
    let resolved =
        serde_json::to_value(resolve_settings_with(host, &options(json!({}))).expect("resolve")).expect("encode");
    let sources = resolved["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .map(|source| source["source"].as_str().expect("source name").to_owned())
        .collect();
    (js_stringify(&resolved["effective"]), sources)
}

#[test]
fn numbers_past_the_double_range_resolve_as_infinity_does_in_the_typescript_resolver() {
    let host = FakeHost::linux()
        .file(
            "/home/fixture/.claude/settings.json",
            r#"{"custom":{"l":[null,1]},"env":{"A":5,"B":true,"C":null,"D":1.5e300,"E":1e400,"F":-1e400,"G":[1,[2,null]],"H":{},"I":1e21,"J":1e-7,"K":-0,"L":0.1,"M":123456789012345678901}}"#,
        )
        .file(
            "/work/project/.claude/settings.json",
            r#"{"custom":{"l":[1e400,-1e400,1e400,null],"x":1e400},"n":-1e400}"#,
        );
    let (effective, sources) = effective_text(&host);
    assert_eq!(
        effective,
        r#"{"env":{"A":"5","B":"true","C":"null","D":"1.5e+300","E":"Infinity","F":"-Infinity","G":"1,2,","H":"[object Object]","I":"1e+21","J":"1e-7","K":"0","L":"0.1","M":"123456789012345680000"},"custom":{"l":[null,1,null,null],"x":null},"n":null}"#
    );
    assert_eq!(sources, ["user", "project"]);
}

#[test]
fn a_typed_setting_refuses_infinity_and_the_file_does_not_load() {
    let host = FakeHost::linux()
        .file("/home/fixture/.claude/settings.json", r#"{"model":1e400}"#)
        .file(
            "/work/project/.claude/settings.json",
            r#"{"cleanupPeriodDays":1e400,"theme":"dark"}"#,
        );
    let (effective, sources) = effective_text(&host);
    assert_eq!(effective, "{}");
    assert!(sources.is_empty(), "{sources:?}");
}

#[test]
fn an_infinite_environment_value_overrides_as_its_string() {
    let host = FakeHost::linux()
        .file("/home/fixture/.claude/settings.json", r#"{"env":{"A":"x"}}"#)
        .file("/work/project/.claude/settings.json", r#"{"env":{"A":1e400,"B":"y"}}"#);
    assert_eq!(effective_text(&host).0, r#"{"env":{"A":"Infinity","B":"y"}}"#);
}

#[test]
fn a_managed_file_with_an_infinite_number_still_loads() {
    let host = FakeHost::linux().file(
        "/etc/claude-code/managed-settings.json",
        r#"{"permissions":{"allow":["Bash"]},"n":1e400}"#,
    );
    let result = resolve(&host, json!({}));
    assert_eq!(
        js_stringify(&result["effective"]),
        r#"{"permissions":{"allow":["Bash"]},"n":null}"#
    );
    assert_eq!(result["machinePolicy"]["didNotLoad"], json!(false));
}
