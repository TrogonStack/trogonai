#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    cell::Cell,
    io,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    Held, held_by_other_tier, pin_machine_login, read_policy_file, restrictive_union_env, same_value, select_admin,
};
use crate::{
    adapters::ReadFile,
    constants::{
        MCP_SERVER_ALLOW_LIST_KEY, PARENT_MANAGED_FILE, REGISTRY_USER_FILE, REMOTE_MANAGED_FILE,
        SETTINGS_FILE_MAX_BYTES,
    },
    node_merge_warmth::MergeWarmth,
    node_stack::NodeStack,
    settings_dialect::SettingsDialect,
    settings_managed_reader::{ManagedReader, ParsedFiles},
    settings_resolve_platform_kind::PlatformKind,
    settings_resolve_tier::{LoadState, TierRead},
    settings_resolve_values::SettingsMap,
};

fn object(value: &Value) -> SettingsMap {
    value.as_object().expect("object").clone()
}

fn loaded(settings: Option<Value>, has_policy_content: bool) -> TierRead {
    TierRead {
        settings: settings.as_ref().map(object),
        has_policy_content,
        load_state: LoadState::Loaded,
        ..TierRead::absent()
    }
}

/// What reading the one managed document answers.
enum Answer {
    Bytes(Vec<u8>),
    Fails(io::ErrorKind),
}

struct OneFile(Answer);

impl ReadFile for OneFile {
    type Error = io::Error;

    fn read_file(&self, _path: &Path) -> Result<Vec<u8>, io::Error> {
        match &self.0 {
            Answer::Bytes(bytes) => Ok(bytes.clone()),
            Answer::Fails(kind) => Err(io::Error::from(*kind)),
        }
    }
}

fn read(answer: Answer) -> (TierRead, bool) {
    let parsed = ParsedFiles::default();
    let warmth = Cell::new(MergeWarmth::default());
    read_policy_file(
        &OneFile(answer),
        &PathBuf::from("/etc/claude-code/managed-settings.json"),
        ManagedReader::new(SettingsDialect::Python, NodeStack::default(), &parsed, &warmth),
    )
}

#[test]
fn a_managed_document_that_cannot_be_read_did_not_load_and_holds_no_settings() {
    let (tier, opened) = read(Answer::Fails(io::ErrorKind::PermissionDenied));
    assert!(!opened);
    assert_eq!(tier.load_state, LoadState::DidNotLoad);
    assert!(tier.unreadable);
    assert!(tier.settings.is_none());

    let (tier, opened) = read(Answer::Fails(io::ErrorKind::NotFound));
    assert!(!opened);
    assert_eq!(tier.load_state, LoadState::Absent);
    assert!(!tier.unreadable);
}

#[test]
fn a_managed_document_over_the_size_limit_did_not_load() {
    let (tier, opened) = read(Answer::Bytes(vec![b' '; SETTINGS_FILE_MAX_BYTES + 1]));
    assert!(!opened);
    assert_eq!(tier.load_state, LoadState::DidNotLoad);
    assert!(tier.unreadable);
    assert!(tier.settings.is_none());
}

#[test]
fn a_blank_managed_document_loads_as_empty_settings() {
    let (tier, opened) = read(Answer::Bytes(b" \n\t ".to_vec()));
    assert!(opened);
    assert_eq!(tier.load_state, LoadState::Loaded);
    assert_eq!(tier.settings, Some(SettingsMap::new()));
    assert!(!tier.has_policy_content);
    assert!(!tier.unreadable);
}

#[test]
fn a_managed_document_that_is_not_an_object_did_not_load_but_was_opened() {
    let (tier, opened) = read(Answer::Bytes(b"[1, 2]".to_vec()));
    assert!(opened);
    assert_eq!(tier.load_state, LoadState::DidNotLoad);
    assert!(!tier.unreadable);
    assert!(tier.settings.is_none());

    let (tier, opened) = read(Answer::Bytes(br#"{"model": "opus"}"#.to_vec()));
    assert!(opened);
    assert_eq!(tier.load_state, LoadState::Loaded);
    assert!(tier.has_policy_content);
    assert_eq!(tier.settings, Some(object(&json!({"model": "opus"}))));
}

#[test]
fn a_managed_document_reads_through_the_policy_schema() {
    let (tier, opened) = read(Answer::Bytes(
        br#"{"model": 5, "custom": {"a": 1}, "permissions": {"defaultMode": 5}}"#.to_vec(),
    ));
    assert!(opened);
    assert_eq!(tier.load_state, LoadState::Loaded);
    assert_eq!(
        tier.settings,
        Some(object(
            &json!({"permissions": {"defaultMode": "default"}, "custom": {"a": 1}})
        ))
    );
    assert!(tier.floored);
    assert!(tier.has_policy_content);

    let (tier, _) = read(Answer::Bytes(
        br#"{"permissions": {"disableBypassPermissionsMode": "bogus"}}"#.to_vec(),
    ));
    assert_eq!(
        tier.settings,
        Some(object(
            &json!({"permissions": {"disableBypassPermissionsMode": "disable"}})
        ))
    );
    assert!(tier.only_substitutes);
    assert!(!tier.floored);
    assert!(!tier.is_authored());

    let (tier, _) = read(Answer::Bytes(br#"{"model": "opus"}"#.to_vec()));
    assert!(!tier.only_substitutes);
    assert!(tier.is_authored());
}

#[test]
fn a_document_handed_over_reads_through_the_policy_schema() {
    let tier = TierRead::from_pm(
        object(&json!({"model": 5})),
        SettingsDialect::TypeScript,
        REMOTE_MANAGED_FILE,
    );
    assert_eq!(tier.load_state, LoadState::Loaded);
    assert_eq!(tier.settings, None);
    assert!(!tier.only_substitutes);

    let tier = TierRead::from_pm(
        object(&json!({"permissions": {"disableBypassPermissionsMode": "bogus"}})),
        SettingsDialect::Python,
        PARENT_MANAGED_FILE,
    );
    assert_eq!(
        tier.settings,
        Some(object(
            &json!({"permissions": {"disableBypassPermissionsMode": "disable"}})
        ))
    );
    assert!(tier.only_substitutes);
    assert!(!tier.is_authored());

    let tier = TierRead::from_hm(r#"{"model": 5, "isolation": {}}"#, true, REGISTRY_USER_FILE);
    assert_eq!(tier.settings, Some(SettingsMap::new()));
    assert!(tier.user_writable);
    assert!(!tier.is_authored());
}

#[test]
fn a_tier_of_only_stand_ins_does_not_shut_the_parent_out() {
    let stand_ins = TierRead {
        only_substitutes: true,
        ..loaded(Some(json!({"model": "opus"})), true)
    };
    let authored = loaded(Some(json!({"model": "opus"})), true);
    let absent = TierRead::absent();
    let held = Held::default();
    assert!(select_admin(Some(&stand_ins), &absent, &absent, &held, None, "linux").parent_never_shut_out);
    assert!(!select_admin(Some(&authored), &absent, &absent, &held, None, "linux").parent_never_shut_out);
    assert!(select_admin(None, &stand_ins, &absent, &held, None, "linux").parent_never_shut_out);
    assert!(!select_admin(None, &authored, &absent, &held, None, "linux").parent_never_shut_out);
}

#[test]
fn missing_values_are_equal_only_to_each_other() {
    assert!(same_value(None, None));
    assert!(same_value(Some(&json!(["a", 1])), Some(&json!(["a", 1.0]))));
    assert!(!same_value(Some(&json!(["a"])), Some(&json!(["b"]))));
    assert!(!same_value(Some(&json!(["a"])), None));
    assert!(!same_value(None, Some(&json!(["a"]))));
}

#[test]
fn merged_tiers_hold_a_key_whenever_another_tier_sets_it() {
    let tiers = vec![
        object(&json!({"allowedMcpServers": [{"serverName": "file"}]})),
        object(&json!({"allowedMcpServers": [{"serverName": "mdm"}]})),
    ];
    assert!(held_by_other_tier(
        &tiers,
        Some(0),
        true,
        MCP_SERVER_ALLOW_LIST_KEY,
        None,
        None
    ));
    assert!(!held_by_other_tier(
        &tiers,
        Some(0),
        true,
        "allowedHttpHookUrls",
        None,
        None
    ));
}

#[test]
fn a_later_tier_holds_the_mcp_allow_list_only_when_servers_are_managed_only() {
    let file = object(&json!({"allowedMcpServers": [{"serverName": "file"}]}));
    let managed_only = object(&json!({
        "allowedMcpServers": [{"serverName": "mdm"}],
        "allowManagedMcpServersOnly": true
    }));
    let open = object(&json!({"allowedMcpServers": [{"serverName": "mdm"}]}));
    assert!(held_by_other_tier(
        &[file.clone(), managed_only],
        Some(0),
        false,
        MCP_SERVER_ALLOW_LIST_KEY,
        None,
        None
    ));
    assert!(!held_by_other_tier(
        &[file, open],
        Some(0),
        false,
        MCP_SERVER_ALLOW_LIST_KEY,
        None,
        None
    ));
}

#[test]
fn a_remote_tier_that_loses_to_an_authored_mdm_tier_ranks_after_it() {
    let remote = object(&json!({"permissions": {}}));
    let mdm = loaded(Some(json!({"model": "opus"})), true);
    let selection = select_admin(
        Some(&loaded(Some(Value::Object(remote.clone())), true)),
        &mdm,
        &TierRead::absent(),
        &Held::default(),
        None,
        "linux",
    );
    assert_eq!(selection.tiers, vec![object(&json!({"model": "opus"})), remote]);
    assert!(!selection.remote_present);
    assert!(selection.mdm_present);
    assert!(selection.remote_in_sources);
}

#[test]
fn an_mdm_tier_that_loses_to_an_authored_file_ranks_after_it() {
    let mdm = loaded(Some(json!({"permissions": {}})), true);
    let file = loaded(Some(json!({"model": "opus"})), true);
    let selection = select_admin(None, &mdm, &file, &Held::default(), None, "linux");
    assert_eq!(
        selection.tiers,
        vec![object(&json!({"model": "opus"})), object(&json!({"permissions": {}}))]
    );
    assert_eq!(selection.file_index, Some(0));
    assert!(!selection.mdm_present);
    assert!(selection.file_present);
}

#[test]
fn a_tier_whose_content_was_all_withheld_still_counts_as_present() {
    let mdm = loaded(None, true);
    let selection = select_admin(None, &mdm, &TierRead::absent(), &Held::default(), None, "linux");
    assert_eq!(selection.admin, Some(SettingsMap::new()));
    assert!(selection.composed.is_none());
    assert!(selection.mdm_present);
    assert!(!selection.file_present);

    let file = loaded(None, true);
    let selection = select_admin(None, &TierRead::absent(), &file, &Held::default(), None, "linux");
    assert_eq!(selection.admin, Some(SettingsMap::new()));
    assert!(!selection.mdm_present);
    assert!(selection.file_present);
}

fn environment(tiers: &[Value], first_trusted_tier: usize) -> Option<Value> {
    let tiers: Vec<SettingsMap> = tiers.iter().map(object).collect();
    restrictive_union_env(&tiers, first_trusted_tier).map(Value::Object)
}

#[test]
fn a_later_tier_with_an_auth_helper_cannot_move_its_provider_variables() {
    let trusted = json!({"env": {"KEEP": "0"}});
    assert_eq!(
        environment(
            &[
                trusted.clone(),
                json!({"apiKeyHelper": "helper", "env": {"ANTHROPIC_BASE_URL": "https://example.test", "API": "1"}}),
            ],
            0
        ),
        Some(json!({"KEEP": "0", "API": "1"}))
    );
    assert_eq!(
        environment(
            &[
                trusted.clone(),
                json!({"awsAuthRefresh": "refresh", "env": {"CLAUDE_CODE_USE_BEDROCK": "1", "ANTHROPIC_BEDROCK_BASE_URL": "https://example.test", "AWS": "1"}}),
            ],
            0
        ),
        Some(json!({"KEEP": "0", "AWS": "1"}))
    );
    assert_eq!(
        environment(
            &[
                trusted.clone(),
                json!({"gcpAuthRefresh": "refresh", "env": {"CLAUDE_CODE_USE_VERTEX": "1", "GCP": "1"}}),
            ],
            0
        ),
        Some(json!({"KEEP": "0", "GCP": "1"}))
    );
    assert_eq!(
        environment(
            &[json!({"gcpAuthRefresh": " ", "env": {"CLAUDE_CODE_USE_VERTEX": "1"}})],
            0
        ),
        Some(json!({"CLAUDE_CODE_USE_VERTEX": "1"}))
    );
}

#[test]
fn a_header_helper_without_an_environment_locks_telemetry_for_later_tiers() {
    assert_eq!(
        environment(
            &[
                json!({"otelHeadersHelper": "helper"}),
                json!({"env": {"OTEL_LOGS_EXPORTER": "otlp", "KEEP": "1"}}),
            ],
            0
        ),
        Some(json!({"KEEP": "1"}))
    );
}

#[test]
fn blank_telemetry_values_are_skipped() {
    assert_eq!(
        environment(&[json!({"env": {"OTEL_LOGS_EXPORTER": "  ", "KEEP": "1"}})], 0),
        Some(json!({"KEEP": "1"}))
    );
    assert_eq!(environment(&[json!({"env": {"OTEL_LOGS_EXPORTER": ""}})], 0), None);
}

#[test]
fn an_untrusted_tier_that_pins_headers_cannot_route_telemetry() {
    assert_eq!(
        environment(
            &[
                json!({"env": {"KEEP": "1"}}),
                json!({"otelHeadersHelper": "helper", "env": {"OTEL_EXPORTER_OTLP_ENDPOINT": "https://example.test", "MORE": "2"}}),
            ],
            0
        ),
        Some(json!({"KEEP": "1", "MORE": "2"}))
    );
}

#[test]
fn the_first_tier_to_route_telemetry_keeps_it() {
    assert_eq!(
        environment(
            &[
                json!({"env": {"OTEL_LOGS_EXPORTER": "otlp"}}),
                json!({"env": {"OTEL_LOG_USER_PROMPTS": "1", "KEEP": "1"}}),
            ],
            0
        ),
        Some(json!({"OTEL_LOGS_EXPORTER": "otlp", "KEEP": "1"}))
    );
}

#[test]
fn variable_names_that_differ_only_in_case_resolve_by_tier() {
    assert_eq!(
        environment(&[json!({"env": {"Foo": "1", "FOO": "2"}})], 0),
        Some(json!({"Foo": "1", "FOO": "2"}))
    );
    assert_eq!(
        environment(
            &[json!({"env": {"BAR": "1"}}), json!({"env": {"bar": "2", "BAR": "3"}}),],
            0
        ),
        Some(json!({"BAR": "1", "bar": "1"}))
    );
}

#[test]
fn a_user_writable_mdm_tier_does_not_pin_the_machine_login() {
    let mdm = TierRead {
        user_writable: true,
        ..loaded(Some(json!({"forceLoginOrgUUID": "mdm-org"})), true)
    };
    let mut merged = SettingsMap::new();
    pin_machine_login(
        &mut merged,
        &mdm,
        &TierRead::absent(),
        &Held::default(),
        None,
        PlatformKind::Linux,
    );
    assert!(merged.is_empty());
}

#[test]
fn the_machine_policy_keeps_the_login_pins_it_sets() {
    let file = loaded(
        Some(json!({"forceLoginOrgUUID": "file-org", "forceLoginMethod": "console"})),
        true,
    );
    let mut merged = object(&json!({"forceLoginOrgUUID": "remote-org"}));
    pin_machine_login(
        &mut merged,
        &TierRead::absent(),
        &file,
        &Held::default(),
        None,
        PlatformKind::Linux,
    );
    assert_eq!(
        Value::Object(merged),
        json!({"forceLoginOrgUUID": "file-org", "forceLoginMethod": "console"})
    );

    let file = loaded(Some(json!({"forceLoginMethod": "sso"})), true);
    let mut merged = SettingsMap::new();
    pin_machine_login(
        &mut merged,
        &TierRead::absent(),
        &file,
        &Held::default(),
        None,
        PlatformKind::Linux,
    );
    assert!(merged.is_empty());
}
