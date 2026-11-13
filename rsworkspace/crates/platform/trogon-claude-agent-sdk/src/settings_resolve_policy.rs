use std::{
    cmp::Ordering,
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    adapters::{DirectoryEntryKind, ReadFile, SettingsHost},
    constants::{
        ALLOW_LIST_KEYS, AUTH_HELPERS, CLOUD_PROVIDER_FLAGS, DISABLE_ADMIN_ENV_UNION_VARIABLE, HELD_ALLOW_LIST_KEYS,
        LOGIN_METHOD_KEY, LOGIN_METHODS, LOGIN_ORGANIZATION_KEY, MANAGED_SETTINGS_DROP_IN_DIRECTORY,
        MANAGED_SETTINGS_FILE, MANAGED_SOURCES_BEHAVIOR_KEY, MANAGED_SOURCES_MERGE, MCP_SERVER_ALLOW_LIST_KEY,
        OTEL_EXACT, OTEL_PREFIXES, PARENT_RESTRICT_FALSE_KEYS, PARENT_RESTRICT_TRUE_KEYS, PARENT_SETTINGS_BEHAVIOR_KEY,
        POLICY_META_KEYS, SETTINGS_FILE_MAX_BYTES,
    },
    settings_resolve_admin::build_admin,
    settings_resolve_parent::{AdminFacts, drop_open_gates, mcp_servers_managed_only, parent_slice},
    settings_resolve_platform::{
        MdmEvaluation, asks_organization, kernel_reports_wsl, managed_directory, wsl_managed_directory,
    },
    settings_resolve_platform_kind::PlatformKind,
    settings_resolve_restrict::{js_equal, restrict_policy_document},
    settings_resolve_tier::{LoadState, TierRead},
    settings_resolve_values::{SettingsMap, has_authored_value, has_policy_content, merge_with_union, omit, pick},
    settings_resolve_writable::{DocumentProbe, user_can_write},
};

fn is_absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

/// One managed settings document read as policy (`lE(..., true)` through `Fu`), and whether the read opened it, which
/// is when upstream holds the owner of the file.
fn read_policy_file<H: ReadFile<Error = io::Error>>(host: &H, path: &Path) -> (TierRead, bool) {
    let bytes = match host.read_file(path) {
        Ok(bytes) => bytes,
        Err(error) if is_absent(&error) => return (TierRead::absent(), false),
        Err(_) => {
            let mut tier = TierRead::did_not_load(true);
            tier.settings = None;
            return (tier, false);
        }
    };
    if bytes.len() > SETTINGS_FILE_MAX_BYTES {
        let mut tier = TierRead::did_not_load(true);
        tier.settings = None;
        return (tier, false);
    }
    let text = String::from_utf8_lossy(&bytes);
    if text.trim().is_empty() {
        let tier = TierRead {
            settings: Some(SettingsMap::new()),
            has_policy_content: false,
            load_state: LoadState::Loaded,
            user_writable: false,
            unreadable: false,
        };
        return (tier, true);
    }
    let tier = match crate::settings_parse::parse_json(&text) {
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
    };
    (tier, true)
}

fn utf16_order(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn is_drop_in(name: &str) -> bool {
    name.ends_with(".json") && !name.starts_with('.')
}

/// Whether the user could rewrite a document `um` upstream reads, as the third argument of its callback tells it.
#[derive(Clone, Copy)]
enum Rewritable<'p> {
    /// A document the read did not find (`() => false`).
    Never,
    /// A document the read found, and where to look to learn about it.
    Ask(&'p DocumentProbe<'p>),
    /// A registry document handed over to be held (`yX`).
    Always,
}

/// The callback `um` upstream hands every document it reads: the settings to merge for that document, given the
/// read and whether the user could rewrite it.
type DocumentFilter<'f> = dyn FnMut(&TierRead, Rewritable<'_>) -> Option<SettingsMap> + 'f;

/// The documents of one managed folder merged so far (`s` in `um` upstream).
struct Folder {
    merged: SettingsMap,
    found: bool,
    content: bool,
    state: LoadState,
    unreadable: bool,
}

impl Folder {
    fn new() -> Self {
        Self {
            merged: SettingsMap::new(),
            found: false,
            content: false,
            state: LoadState::Absent,
            unreadable: false,
        }
    }

    fn absorb(&mut self, document: &TierRead, settings: Option<SettingsMap>) {
        if let Some(settings) = settings
            && !settings.is_empty()
        {
            merge_with_union(&mut self.merged, &settings);
            self.found = true;
        }
        self.content |= document.holds_policy_content();
        self.unreadable |= document.unreadable;
        self.state = self.state.combine(document.load_state);
    }

    /// `rX` upstream: the tier the merged documents make.
    fn finish(self) -> TierRead {
        TierRead {
            settings: (self.found && has_policy_content(&self.merged)).then_some(self.merged),
            has_policy_content: self.content,
            load_state: self.state,
            user_writable: false,
            unreadable: self.unreadable,
        }
    }
}

/// `um` upstream: the held registry documents, then the managed settings file of `directory` plus its sorted
/// drop-ins, merged after `filter` saw each.
fn read_managed_directory<H: SettingsHost>(
    host: &H,
    directory: Option<&Path>,
    held: &[TierRead],
    filter: &mut DocumentFilter<'_>,
) -> TierRead {
    let mut folder = Folder::new();
    for document in held {
        let settings = filter(document, Rewritable::Always);
        folder.absorb(document, settings);
    }
    let Some(directory) = directory else {
        return folder.finish();
    };
    let (document, opened) = read_policy_file(host, &directory.join(MANAGED_SETTINGS_FILE));
    let probe = (document.load_state != LoadState::Absent).then(|| DocumentProbe {
        root: directory,
        components: vec![MANAGED_SETTINGS_FILE.to_owned()],
        last_is_link: None,
        opened,
    });
    let settings = filter(&document, probe.as_ref().map_or(Rewritable::Never, Rewritable::Ask));
    folder.absorb(&document, settings);
    let drop_ins = directory.join(MANAGED_SETTINGS_DROP_IN_DIRECTORY);
    match host.read_directory(&drop_ins) {
        Ok(entries) => {
            let mut listed: Vec<(String, bool)> = entries
                .into_iter()
                .filter(|entry| matches!(entry.kind, DirectoryEntryKind::File | DirectoryEntryKind::Symlink))
                .filter(|entry| is_drop_in(&entry.name))
                .map(|entry| (entry.name, entry.kind == DirectoryEntryKind::Symlink))
                .collect();
            listed.sort_by(|left, right| utf16_order(&left.0, &right.0));
            for (name, linked) in listed {
                let (document, opened) = read_policy_file(host, &drop_ins.join(&name));
                let probe = (document.load_state != LoadState::Absent).then(|| DocumentProbe {
                    root: directory,
                    components: vec![MANAGED_SETTINGS_DROP_IN_DIRECTORY.to_owned(), name],
                    last_is_link: Some(linked),
                    opened,
                });
                let settings = filter(&document, probe.as_ref().map_or(Rewritable::Never, Rewritable::Ask));
                folder.absorb(&document, settings);
            }
        }
        Err(error) if is_absent(&error) => {}
        Err(_) => {
            folder.state = LoadState::DidNotLoad;
            folder.unreadable = true;
        }
    }
    folder.finish()
}

/// `Q5` upstream: the managed file tier for this platform, with the held registry documents ahead of the managed
/// files. A session that asks its organization reads the Windows side of WSL only when the kernel says WSL.
fn read_file_tier<H: SettingsHost>(
    host: &H,
    kind: PlatformKind,
    mdm: &MdmEvaluation,
    organization: bool,
    filter: &mut DocumentFilter<'_>,
) -> TierRead {
    let held = mdm.held_documents.as_slice();
    let local = managed_directory(kind);
    if kind != PlatformKind::Wsl || (organization && !kernel_reports_wsl(host)) {
        return read_managed_directory(host, Some(&local), held, filter);
    }
    let machine_holds = mdm.mdm.has_policy_content || mdm.mdm.load_state == LoadState::DidNotLoad;
    if mdm.wsl_inherits {
        let windows = wsl_managed_directory();
        let windows_keeps_held = (!held.is_empty()).then(|| {
            machine_holds
                || read_managed_directory(
                    host,
                    Some(&windows),
                    &[],
                    &mut |document: &TierRead, _: Rewritable<'_>| document.settings.clone(),
                )
                .lo()
        });
        let windows_held: &[TierRead] = if windows_keeps_held == Some(true) { held } else { &[] };
        let windows_side = read_managed_directory(host, Some(&windows), windows_held, filter);
        if windows_keeps_held.unwrap_or_else(|| windows_side.lo() || machine_holds) {
            return windows_side;
        }
        let mut local_tier = read_managed_directory(host, Some(&local), held, filter);
        local_tier.load_state = windows_side.load_state.combine(local_tier.load_state);
        return local_tier;
    }
    if machine_holds {
        return read_managed_directory(host, None, held, filter);
    }
    read_managed_directory(host, Some(&local), held, filter)
}

/// What reading the managed file for an organization (`pE` upstream) tells the admin selection.
#[derive(Clone, Debug, Default)]
pub(crate) struct Held {
    /// The documents the user cannot rewrite hold no authored value (`heldOnly`).
    held_only: bool,
    /// The file tier holds only what restricted documents contributed (`onlyHeldContent`).
    only_held_content: bool,
    /// Allow lists only the restricted documents set (`heldAllowLists`).
    allow_lists: Vec<&'static str>,
}

/// Whether the user could rewrite a managed document, asked only of a session that asks its organization (`n` in
/// `pE` upstream).
struct Rewrites<'h, H> {
    host: &'h H,
    kind: PlatformKind,
    organization: bool,
    wsl_organization: bool,
    certain: HashMap<PathBuf, bool>,
}

impl<H: SettingsHost> Rewrites<'_, H> {
    fn allowed(&mut self, rewritable: Rewritable<'_>) -> bool {
        match rewritable {
            Rewritable::Always => true,
            Rewritable::Never => false,
            Rewritable::Ask(probe) => {
                self.organization
                    && user_can_write(self.host, self.kind, self.wsl_organization, probe, &mut self.certain)
            }
        }
    }
}

/// `pE` upstream: the managed file tier, where a document the user could rewrite only restricts when the session
/// asks its organization.
fn read_managed_file<H: SettingsHost>(host: &H, kind: PlatformKind, mdm: &MdmEvaluation) -> (TierRead, Held) {
    let organization = asks_organization(host);
    let mut writes = Rewrites {
        host,
        kind,
        organization,
        wsl_organization: organization && kind == PlatformKind::Wsl && kernel_reports_wsl(host),
        certain: HashMap::new(),
    };
    let mut held_meta = SettingsMap::new();
    let mut restricts = false;
    let held = read_file_tier(
        host,
        kind,
        mdm,
        organization,
        &mut |document: &TierRead, rewritable: Rewritable<'_>| {
            let settings = document.settings.as_ref()?;
            if !writes.allowed(rewritable) {
                for (key, value) in pick(settings, &POLICY_META_KEYS) {
                    held_meta.insert(key, value);
                }
                return Some(settings.clone());
            }
            restricts = true;
            Some(SettingsMap::new())
        },
    );
    if !restricts {
        return (held, Held::default());
    }
    let restricted = read_file_tier(
        host,
        kind,
        mdm,
        organization,
        &mut |document: &TierRead, rewritable: Rewritable<'_>| {
            let settings = document.settings.as_ref()?;
            if !writes.allowed(rewritable) {
                return None;
            }
            Some(restrict_policy_document(settings).unwrap_or_else(|| settings.clone()))
        },
    );
    let kept = held.settings.clone().unwrap_or_default();
    let added = restricted.settings.clone().unwrap_or_default();
    let mut flags = Held {
        held_only: !has_authored_value(&kept),
        ..Held::default()
    };
    if added.is_empty() {
        return (held, flags);
    }
    let only_added = kept.is_empty();
    let mut settings = if only_added {
        added.clone()
    } else {
        let composed = build_admin(&[kept.clone(), added.clone()], true, 0, kind.as_str()).unwrap_or_default();
        omit(&composed, &[PARENT_SETTINGS_BEHAVIOR_KEY])
    };
    for (key, value) in pick(&kept, &[PARENT_SETTINGS_BEHAVIOR_KEY])
        .into_iter()
        .chain(held_meta)
    {
        settings.insert(key, value);
    }
    flags.only_held_content = !has_authored_value(&kept);
    flags.allow_lists = HELD_ALLOW_LIST_KEYS
        .iter()
        .copied()
        .filter(|key| added.contains_key(*key) && !kept.contains_key(*key))
        .collect();
    let mut tier = if only_added { restricted } else { held };
    tier.settings = Some(settings);
    (tier, flags)
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

/// lodash `isEqual` over two values either of which may be missing.
fn same_value(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => js_equal(left, right),
        _ => false,
    }
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

/// What `$u` upstream settles about the admin tiers.
struct Selection {
    /// The admin settings, or an empty map when only an empty document holds the tier (`Ar` upstream).
    admin: Option<SettingsMap>,
    /// The admin settings the tiers compose (`co` upstream).
    composed: Option<SettingsMap>,
    /// The tiers merged instead of the first winning (`dl` upstream).
    merged: bool,
    /// Where the file tier sits among `tiers` (`Ie` upstream).
    file_index: Option<usize>,
    remote_present: bool,
    mdm_present: bool,
    file_present: bool,
    tiers: Vec<SettingsMap>,
    /// The parent's restrictive keys apply whatever the admin says (`ln` upstream).
    parent_never_shut_out: bool,
    /// The file tier holds only what the organization withholds, so the parent stays in force (`Ye` upstream).
    held_back: bool,
    remote_in_sources: bool,
}

impl Selection {
    fn held_by_other_tier(&self, key: &str, slot: Option<&SettingsMap>, parent: Option<&SettingsMap>) -> bool {
        held_by_other_tier(&self.tiers, self.file_index, self.merged, key, slot, parent)
    }
}

/// `Us` upstream: a tier other than the file sets `key`.
fn other_tier_sets(tiers: &[SettingsMap], file_index: Option<usize>, key: &str) -> bool {
    tiers
        .iter()
        .enumerate()
        .any(|(index, tier)| Some(index) != file_index && tier.contains_key(key))
}

/// `wm` upstream: a tier other than the file decides `key`, so a document the organization withholds cannot.
fn held_by_other_tier(
    tiers: &[SettingsMap],
    file_index: Option<usize>,
    merged: bool,
    key: &str,
    slot: Option<&SettingsMap>,
    parent: Option<&SettingsMap>,
) -> bool {
    if merged {
        return other_tier_sets(tiers, file_index, key);
    }
    (file_index != Some(0) && tiers.first().is_some_and(|tier| tier.contains_key(key)))
        || (key == MCP_SERVER_ALLOW_LIST_KEY
            && other_tier_sets(tiers, file_index, key)
            && mcp_servers_managed_only(slot, tiers, parent))
}

fn select_admin(
    remote: Option<&SettingsMap>,
    mdm: &TierRead,
    file: &TierRead,
    held: &Held,
    parent: Option<&SettingsMap>,
    platform: &str,
) -> Selection {
    let mdm_settings = mdm.settings.as_ref().filter(|settings| !settings.is_empty());
    let remote_candidate = nonempty(remote);
    let mdm_candidate = nonempty(mdm_settings);
    let file_candidate = nonempty(file.settings.as_ref());
    let remote_authored = remote.is_some_and(has_authored_value);
    let mdm_authored = mdm_settings.is_some_and(has_authored_value);
    let file_authored = file.is_authored();
    let remote_score = score(remote_candidate, remote_authored);
    let mdm_score = score(mdm_candidate, mdm_authored);
    let file_score = if held.only_held_content {
        0
    } else {
        score(file_candidate, file_authored)
    };
    let remote_wins = remote_candidate.is_some() && remote_score >= mdm_score.max(file_score);
    let mdm_wins = mdm_candidate.is_some() && mdm_score >= file_score;
    let remote_demoted = remote_candidate.is_some() && !remote_wins;
    let head_remote = if remote_demoted { None } else { remote_candidate };
    let mdm_demoted = mdm_candidate.is_some() && !mdm_wins && head_remote.is_none();
    let head_mdm = if mdm_demoted { None } else { mdm_candidate };
    let shuts_out = (remote_authored && head_remote.is_some()) || (mdm_authored && head_mdm.is_some());
    let never_shut_out = !shuts_out && (!file_authored || held.held_only);
    let held_back = never_shut_out && file_authored && held.held_only;
    let file_tier = file_candidate.map(|settings| match parent {
        Some(parent) if held_back => {
            let set: Vec<&str> = ALLOW_LIST_KEYS
                .iter()
                .copied()
                .filter(|key| parent.contains_key(*key))
                .collect();
            omit(settings, &set)
        }
        _ => settings.clone(),
    });
    let mut tiers: Vec<SettingsMap> = Vec::new();
    for tier in [head_remote, head_mdm].into_iter().flatten() {
        tiers.push(tier.clone());
    }
    let file_index = file_tier.as_ref().map(|_| tiers.len());
    if let Some(tier) = &file_tier {
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
    let withheld_only = file_index.is_some() && held.only_held_content && !shuts_out && !merge && file_index != Some(0);
    let admin = if withheld_only {
        let others: Vec<SettingsMap> = tiers
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != file_index)
            .map(|(_, tier)| tier.clone())
            .collect();
        build_admin(&others, merge, tiers.len().saturating_sub(2), platform)
    } else {
        build_admin(&tiers, merge, demoted, platform)
    };
    let merged = !withheld_only && merge && tiers.len() >= 2;
    let composed = match (admin, file_tier.as_ref()) {
        (Some(admin), Some(file_tier)) if withheld_only => {
            let decided: Vec<&str> = HELD_ALLOW_LIST_KEYS
                .iter()
                .copied()
                .filter(|key| held_by_other_tier(&tiers, file_index, merged, key, Some(&admin), parent))
                .collect();
            let narrowing = omit(file_tier, &decided);
            build_admin(&[admin, narrowing], true, 0, platform)
        }
        (admin, _) => admin,
    };
    let held_empty = composed.is_none();
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
        admin: composed.clone().or_else(|| first_empty.map(|_| SettingsMap::new())),
        composed,
        merged,
        file_index,
        tiers,
        remote_in_sources: remote_candidate.is_some(),
        parent_never_shut_out: never_shut_out,
        held_back,
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
fn pin_machine_login(
    merged: &mut SettingsMap,
    mdm: &TierRead,
    file: &TierRead,
    held: &Held,
    parent: Option<&SettingsMap>,
    kind: PlatformKind,
) {
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
    let machine = select_admin(None, &machine_mdm, file, held, parent, kind.as_str()).admin;
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
    kind: PlatformKind,
    parent_managed: Option<&SettingsMap>,
    remote: Option<&SettingsMap>,
    evaluation: &MdmEvaluation,
) -> PolicyResolution {
    let (file, held) = read_managed_file(host, kind, evaluation);
    let parent_managed = parent_managed.filter(|parent| !parent.is_empty());
    let remote = remote.filter(|settings| !settings.is_empty());
    let selection = select_admin(remote, &evaluation.mdm, &file, &held, parent_managed, kind.as_str());
    let admin = selection.admin.clone();
    let parent_included = selection.composed.as_ref().is_none_or(|composed| {
        composed.get(PARENT_SETTINGS_BEHAVIOR_KEY) == Some(&Value::String(MANAGED_SOURCES_MERGE.into()))
    }) || selection.parent_never_shut_out;
    let mut facts = AdminFacts::new(admin.as_ref(), &selection.tiers, parent_managed);
    let file_candidate = nonempty(file.settings.as_ref());
    let released: Vec<&str> = match parent_managed {
        Some(parent) if parent_included && !selection.held_back => ALLOW_LIST_KEYS
            .iter()
            .copied()
            .filter(|key| {
                parent.contains_key(*key)
                    && held.allow_lists.contains(key)
                    && same_value(facts.allow_list(key), file_candidate.and_then(|file| file.get(*key)))
                    && !selection.held_by_other_tier(key, admin.as_ref(), parent_managed)
            })
            .collect(),
        _ => Vec::new(),
    };
    let slice = match parent_managed {
        Some(parent) if parent_included => {
            let slice = if selection.held_back {
                let gated = drop_open_gates(parent, &facts);
                facts.hold_everything();
                parent_slice(&gated, &facts)
            } else {
                facts.release(&released);
                parent_slice(parent, &facts)
            };
            (!slice.is_empty()).then_some(slice)
        }
        _ => None,
    };
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
        let kept = admin.as_ref().map(|admin| omit(admin, &released)).unwrap_or_default();
        merge_with_union(&mut merged, &kept);
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
            pin_machine_login(&mut merged, &evaluation.mdm, &file, &held, parent_managed, kind);
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
