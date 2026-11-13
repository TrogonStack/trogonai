use std::{cmp::Ordering, io, path::Path};

use serde_json::Value;

use crate::{
    adapters::{DirectoryEntryKind, ReadFile, SettingsHost},
    constants::{AUTH_HELPERS, CLOUD_PROVIDER_FLAGS, OTEL_EXACT, OTEL_PREFIXES},
    constants::{
        DISABLE_ADMIN_ENV_UNION_VARIABLE, LOGIN_METHOD_KEY, LOGIN_METHODS, LOGIN_ORGANIZATION_KEY,
        MANAGED_SETTINGS_DROP_IN_DIRECTORY, MANAGED_SETTINGS_FILE, MANAGED_SOURCES_BEHAVIOR_KEY, MANAGED_SOURCES_MERGE,
        PARENT_RESTRICT_FALSE_KEYS, PARENT_RESTRICT_TRUE_KEYS, SETTINGS_FILE_MAX_BYTES,
    },
    settings_resolve_admin::build_admin,
    settings_resolve_parent::parent_slice,
    settings_resolve_platform::{MdmEvaluation, PlatformKind, managed_directory, platform_kind, wsl_managed_directory},
    settings_resolve_tier::{LoadState, TierRead},
    settings_resolve_values::{SettingsMap, has_authored_value, has_policy_content, merge_with_union},
};

fn is_absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

/// One managed settings document read as policy (`lE(..., true)` through `Fu`).
fn read_policy_file<H: ReadFile<Error = io::Error>>(host: &H, path: &Path) -> TierRead {
    let bytes = match host.read_file(path) {
        Ok(bytes) => bytes,
        Err(error) if is_absent(&error) => return TierRead::absent(),
        Err(_) => {
            let mut tier = TierRead::did_not_load(true);
            tier.settings = None;
            return tier;
        }
    };
    if bytes.len() > SETTINGS_FILE_MAX_BYTES {
        let mut tier = TierRead::did_not_load(true);
        tier.settings = None;
        return tier;
    }
    let text = String::from_utf8_lossy(&bytes);
    if text.trim().is_empty() {
        return TierRead {
            settings: Some(SettingsMap::new()),
            has_policy_content: false,
            load_state: LoadState::Loaded,
            user_writable: false,
            unreadable: false,
        };
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(document)) => TierRead {
            has_policy_content: has_policy_content(&document),
            settings: Some(document),
            load_state: LoadState::Loaded,
            user_writable: false,
            unreadable: false,
        },
        _ => {
            let mut tier = TierRead::did_not_load(false);
            tier.settings = None;
            tier
        }
    }
}

fn utf16_order(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn is_drop_in(name: &str) -> bool {
    name.ends_with(".json") && !name.starts_with('.')
}

/// `um` upstream: the managed settings file of `directory` plus its sorted drop-ins, merged.
fn read_managed_directory<H: SettingsHost>(host: &H, directory: Option<&Path>) -> TierRead {
    let mut merged = SettingsMap::new();
    let mut found = false;
    let mut content = false;
    let mut state = LoadState::Absent;
    let mut unreadable = false;
    let Some(directory) = directory else {
        return TierRead::absent();
    };
    let mut absorb = |document: TierRead| {
        if let Some(settings) = &document.settings
            && !settings.is_empty()
        {
            merge_with_union(&mut merged, settings);
            found = true;
        }
        content |= document.holds_policy_content();
        unreadable |= document.unreadable;
        state = state.combine(document.load_state);
    };
    absorb(read_policy_file(host, &directory.join(MANAGED_SETTINGS_FILE)));
    let drop_ins = directory.join(MANAGED_SETTINGS_DROP_IN_DIRECTORY);
    match host.read_directory(&drop_ins) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .into_iter()
                .filter(|entry| matches!(entry.kind, DirectoryEntryKind::File | DirectoryEntryKind::Symlink))
                .map(|entry| entry.name)
                .filter(|name| is_drop_in(name))
                .collect();
            names.sort_by(|left, right| utf16_order(left, right));
            for name in names {
                absorb(read_policy_file(host, &drop_ins.join(name)));
            }
        }
        Err(error) if is_absent(&error) => {}
        Err(_) => {
            state = LoadState::DidNotLoad;
            unreadable = true;
        }
    }
    TierRead {
        settings: (found && has_policy_content(&merged)).then_some(merged),
        has_policy_content: content,
        load_state: state,
        user_writable: false,
        unreadable,
    }
}

/// `Q5` upstream: the managed file tier for this platform.
fn read_file_tier<H: SettingsHost>(host: &H, kind: PlatformKind, mdm: &TierRead, wsl_inherits: bool) -> TierRead {
    let local = managed_directory(kind);
    if kind != PlatformKind::Wsl {
        return read_managed_directory(host, Some(&local));
    }
    let machine_holds = mdm.has_policy_content || mdm.load_state == LoadState::DidNotLoad;
    if wsl_inherits {
        let windows_side = read_managed_directory(host, Some(&wsl_managed_directory()));
        if windows_side.lo() || machine_holds {
            return windows_side;
        }
        let mut local_tier = read_managed_directory(host, Some(&local));
        local_tier.load_state = windows_side.load_state.combine(local_tier.load_state);
        return local_tier;
    }
    if machine_holds {
        return read_managed_directory(host, None);
    }
    read_managed_directory(host, Some(&local))
}

/// Where the winning policy tier came from (`IX` upstream).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PolicyOrigin {
    Remote,
    Plist,
    Hklm,
    File,
    Parent,
    Hkcu,
}

impl PolicyOrigin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Plist => "plist",
            Self::Hklm => "hklm",
            Self::File => "file",
            Self::Parent => "parent",
            Self::Hkcu => "hkcu",
        }
    }
}

/// The policy tier the resolver merges last, with what `machinePolicy` reports.
pub(crate) struct PolicyResolution {
    pub(crate) settings: Option<SettingsMap>,
    pub(crate) origin: Option<PolicyOrigin>,
    pub(crate) mdm_did_not_load: bool,
    pub(crate) file_did_not_load: bool,
}

fn score(settings: Option<&SettingsMap>, authored: bool) -> i32 {
    match settings {
        None => -1,
        Some(_) if authored => 2,
        Some(settings) => i32::from(has_authored_value(settings)),
    }
}

fn nonempty(settings: Option<&SettingsMap>) -> Option<&SettingsMap> {
    settings.filter(|settings| has_policy_content(settings))
}

struct Selection {
    admin: Option<SettingsMap>,
    remote_present: bool,
    mdm_present: bool,
    file_present: bool,
    tiers: Vec<SettingsMap>,
    track_no_ln: bool,
    remote_in_sources: bool,
}

fn select_admin(remote: Option<&SettingsMap>, mdm: &TierRead, file: &TierRead, platform: &str) -> Selection {
    let mdm_settings = mdm.settings.as_ref().filter(|settings| !settings.is_empty());
    let remote_candidate = nonempty(remote);
    let mdm_candidate = nonempty(mdm_settings);
    let file_candidate = nonempty(file.settings.as_ref());
    let remote_authored = remote.is_some_and(has_authored_value);
    let mdm_authored = mdm_settings.is_some_and(has_authored_value);
    let file_authored = file.is_authored();
    let remote_score = score(remote_candidate, remote_authored);
    let mdm_score = score(mdm_candidate, mdm_authored);
    let file_score = score(file_candidate, file_authored);
    let remote_wins = remote_candidate.is_some() && remote_score >= mdm_score.max(file_score);
    let mdm_wins = mdm_candidate.is_some() && mdm_score >= file_score;
    let remote_demoted = remote_candidate.is_some() && !remote_wins;
    let head_remote = if remote_demoted { None } else { remote_candidate };
    let mdm_demoted = mdm_candidate.is_some() && !mdm_wins && head_remote.is_none();
    let head_mdm = if mdm_demoted { None } else { mdm_candidate };
    let mut tiers: Vec<SettingsMap> = Vec::new();
    for tier in [head_remote, head_mdm, file_candidate].into_iter().flatten() {
        tiers.push(tier.clone());
    }
    if remote_demoted && let Some(tier) = remote_candidate {
        tiers.push(tier.clone());
    }
    if mdm_demoted && let Some(tier) = mdm_candidate {
        tiers.push(tier.clone());
    }
    let slots = [
        (remote, remote_wins),
        (mdm_settings, mdm_wins),
        (file.settings.as_ref(), file_candidate.is_some()),
    ];
    let merge = slots
        .iter()
        .find(|&&(raw, wins)| raw.is_some_and(|raw| wins || raw.contains_key(MANAGED_SOURCES_BEHAVIOR_KEY)))
        .and_then(|&(raw, _)| raw.and_then(|raw| raw.get(MANAGED_SOURCES_BEHAVIOR_KEY)))
        .and_then(Value::as_str)
        == Some(MANAGED_SOURCES_MERGE);
    let demoted = usize::from(remote_demoted) + usize::from(mdm_demoted);
    let admin = build_admin(&tiers, merge, demoted, platform);
    let held_empty = admin.is_none();
    let empty_remote = held_empty && remote.is_some_and(has_policy_content);
    let empty_mdm = held_empty && mdm.holds_policy_content();
    let empty_file = held_empty && file.holds_policy_content();
    let first_empty = if empty_remote {
        Some(0)
    } else if empty_mdm {
        Some(1)
    } else if empty_file {
        Some(2)
    } else {
        None
    };
    Selection {
        remote_present: head_remote.is_some() || first_empty == Some(0),
        mdm_present: head_mdm.is_some() || first_empty == Some(1),
        file_present: file_candidate.is_some() || first_empty == Some(2),
        admin: admin.or_else(|| first_empty.map(|_| SettingsMap::new())),
        tiers,
        remote_in_sources: remote_candidate.is_some(),
        track_no_ln: remote_authored && head_remote.is_some() || mdm_authored && head_mdm.is_some(),
    }
}

fn helper_demoted_names(helper: &str) -> &'static [&'static str] {
    match helper {
        "apiKeyHelper" => &["ANTHROPIC_BASE_URL", "_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL"],
        "awsAuthRefresh" | "awsCredentialExport" => &[
            CLOUD_PROVIDER_FLAGS[0],
            CLOUD_PROVIDER_FLAGS[1],
            CLOUD_PROVIDER_FLAGS[2],
            "ANTHROPIC_BEDROCK_BASE_URL",
            "ANTHROPIC_AWS_BASE_URL",
            "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
        ],
        _ => &[
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
            "ANTHROPIC_VERTEX_BASE_URL",
            "ANTHROPIC_GOOGLE_CLOUD_BASE_URL",
        ],
    }
}

/// `ZX` upstream: telemetry-routing variables.
fn is_telemetry_variable(upper: &str) -> bool {
    OTEL_PREFIXES.iter().any(|prefix| upper.starts_with(prefix)) || OTEL_EXACT.contains(&upper)
}

/// `zQe` upstream: the restrictive union of the admin tiers' `env` blocks.
fn restrictive_union_env(tiers: &[SettingsMap], first_trusted_tier: usize) -> Option<SettingsMap> {
    let mut by_upper: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    let mut result: SettingsMap = SettingsMap::new();
    let mut telemetry_locked = false;
    for (index, tier) in tiers.iter().enumerate() {
        let mut seen_here: std::collections::HashSet<String> = std::collections::HashSet::new();
        let pins_headers = tier
            .get("otelHeadersHelper")
            .and_then(Value::as_str)
            .is_some_and(|helper| !helper.trim().is_empty());
        let mut tier_locks_telemetry = pins_headers;
        let demoted: std::collections::HashSet<&str> = AUTH_HELPERS
            .iter()
            .filter(|helper| {
                tier.get(**helper)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
            })
            .flat_map(|helper| helper_demoted_names(helper).iter().copied())
            .collect();
        let Some(Value::Object(environment)) = tier.get("env") else {
            if tier_locks_telemetry {
                telemetry_locked = true;
            }
            continue;
        };
        for (name, value) in environment {
            let upper = name.to_uppercase();
            let telemetry = is_telemetry_variable(&upper);
            let text = value.as_str();
            if telemetry && text.is_some_and(|text| text.trim().is_empty()) {
                continue;
            }
            if telemetry && telemetry_locked {
                continue;
            }
            if telemetry && index > first_trusted_tier && pins_headers {
                continue;
            }
            if let Some(existing) = by_upper.get(&upper) {
                if seen_here.contains(&upper) {
                    result.insert(name.clone(), value.clone());
                } else if !result.contains_key(name) {
                    result.insert(name.clone(), existing.clone());
                }
                continue;
            }
            if index > first_trusted_tier && demoted.contains(upper.as_str()) {
                continue;
            }
            result.insert(name.clone(), value.clone());
            seen_here.insert(upper.clone());
            by_upper.insert(upper, value.clone());
            if telemetry {
                tier_locks_telemetry = true;
            }
        }
        if tier_locks_telemetry {
            telemetry_locked = true;
        }
    }
    (!result.is_empty()).then_some(result)
}

/// `mX` upstream: the machine policy, read without the remote tier, keeps the login pins it sets.
fn pin_machine_login(merged: &mut SettingsMap, mdm: &TierRead, file: &TierRead, kind: PlatformKind) {
    let machine_mdm = if mdm.user_writable {
        TierRead {
            settings: Some(SettingsMap::new()),
            has_policy_content: false,
            load_state: LoadState::Loaded,
            user_writable: false,
            unreadable: false,
        }
    } else {
        mdm.clone()
    };
    let machine = select_admin(None, &machine_mdm, file, kind.as_str()).admin;
    let Some(machine) = machine else {
        return;
    };
    if let Some(organization) = machine.get(LOGIN_ORGANIZATION_KEY) {
        merged.insert(LOGIN_ORGANIZATION_KEY.to_owned(), organization.clone());
    }
    if let Some(method) = machine.get(LOGIN_METHOD_KEY)
        && method.as_str().is_some_and(|name| LOGIN_METHODS.contains(&name))
    {
        merged.insert(LOGIN_METHOD_KEY.to_owned(), method.clone());
    }
}

/// `QP` upstream for the default configuration: no policy helper, no host-managed provider.
pub(crate) fn resolve_policy<H: SettingsHost>(
    host: &H,
    parent_managed: Option<&SettingsMap>,
    remote: Option<&SettingsMap>,
    evaluation: &MdmEvaluation,
) -> PolicyResolution {
    let kind = platform_kind(host);
    let file = read_file_tier(host, kind, &evaluation.mdm, evaluation.wsl_inherits);
    let remote = remote.filter(|settings| !settings.is_empty());
    let selection = select_admin(remote, &evaluation.mdm, &file, kind.as_str());
    let admin = selection.admin.clone();
    let parent_included = admin.is_none()
        || admin.as_ref().and_then(|admin| admin.get("parentSettingsBehavior")) == Some(&Value::String("merge".into()))
        || (!selection.track_no_ln && !file.is_authored());
    let slice = parent_managed
        .filter(|_| parent_included)
        .map(|parent| parent_slice(parent, admin.as_ref(), &selection.tiers))
        .filter(|slice| !slice.is_empty());
    let feature_overlay = slice.as_ref().filter(|slice| {
        admin.is_none()
            && slice.keys().all(|key| {
                PARENT_RESTRICT_TRUE_KEYS.contains(&key.as_str()) || PARENT_RESTRICT_FALSE_KEYS.contains(&key.as_str())
            })
            && !evaluation.hkcu.is_empty()
    });
    let feature_overlay = feature_overlay.cloned();
    let slice = if feature_overlay.is_some() { None } else { slice };
    let mdm_origin = if kind == PlatformKind::Macos {
        PolicyOrigin::Plist
    } else {
        PolicyOrigin::Hklm
    };
    let (settings, origin) = if admin.is_none() && slice.is_none() {
        if evaluation.hkcu.is_empty() {
            (None, None)
        } else {
            let mut settings = evaluation.hkcu.clone();
            if let Some(overlay) = &feature_overlay {
                for (key, value) in overlay {
                    settings.insert(key.clone(), value.clone());
                }
            }
            (Some(settings), Some(PolicyOrigin::Hkcu))
        }
    } else {
        let mut merged = SettingsMap::new();
        merge_with_union(&mut merged, slice.as_ref().unwrap_or(&SettingsMap::new()));
        merge_with_union(&mut merged, admin.as_ref().unwrap_or(&SettingsMap::new()));
        if let Some(slice) = &slice {
            for key in ["availableModelsMatch"] {
                if slice.get(key) == Some(&Value::String("exact".into())) {
                    merged.insert(key.to_owned(), Value::String("exact".into()));
                }
            }
            for key in [
                "allowManagedHooksOnly",
                "disableSkillShellExecution",
                "disableWindowsBashCommandSplit",
            ]
            .iter()
            .chain(PARENT_RESTRICT_TRUE_KEYS.iter())
            {
                if slice.get(*key) == Some(&Value::Bool(true)) {
                    merged.insert((*key).to_owned(), Value::Bool(true));
                }
            }
            for key in PARENT_RESTRICT_FALSE_KEYS {
                if slice.get(key) == Some(&Value::Bool(false)) {
                    merged.insert(key.to_owned(), Value::Bool(false));
                }
            }
        }
        if selection
            .tiers
            .iter()
            .any(|tier| tier.get("forceRemoteSettingsRefresh") == Some(&Value::Bool(true)))
        {
            merged.insert("forceRemoteSettingsRefresh".to_owned(), Value::Bool(true));
        }
        if selection.remote_in_sources || evaluation.mdm.user_writable {
            pin_machine_login(&mut merged, &evaluation.mdm, &file, kind);
        }
        let union_disabled = crate::settings_resolve_env::flag_is_true(host, DISABLE_ADMIN_ENV_UNION_VARIABLE);
        if !union_disabled {
            match restrictive_union_env(&selection.tiers, 0) {
                Some(environment) => {
                    merged.insert("env".to_owned(), Value::Object(environment));
                }
                None => {
                    merged.shift_remove("env");
                }
            }
        }
        if let Some(Value::Object(environment)) = merged.get("env") {
            let kept: SettingsMap = environment
                .iter()
                .filter(|(name, _)| name.to_uppercase() != DISABLE_ADMIN_ENV_UNION_VARIABLE)
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            if kept.is_empty() {
                merged.shift_remove("env");
            } else {
                merged.insert("env".to_owned(), Value::Object(kept));
            }
        }
        let origin = if selection.remote_present {
            Some(PolicyOrigin::Remote)
        } else if selection.mdm_present {
            Some(mdm_origin)
        } else if selection.file_present {
            Some(PolicyOrigin::File)
        } else {
            slice.as_ref().map(|_| PolicyOrigin::Parent)
        };
        (Some(merged), origin)
    };
    let mdm_did_not_load = !evaluation.mdm.user_writable
        && (evaluation.mdm.load_state == LoadState::DidNotLoad || evaluation.mdm.unreadable);
    PolicyResolution {
        settings,
        origin,
        mdm_did_not_load,
        file_did_not_load: file.load_state == LoadState::DidNotLoad,
    }
}
