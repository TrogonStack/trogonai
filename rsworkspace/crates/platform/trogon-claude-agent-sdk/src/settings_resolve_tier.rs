use serde_json::Value;

use crate::{
    constants::MANAGED_ONLY_KEYS,
    settings_parse::parse_json,
    settings_parse_deep::{nests_past_serde_json, parse_json_deep},
    settings_policy_content::{has_authored_value, has_policy_content},
    settings_resolve_values::SettingsMap,
    settings_strings::stand_in_escapes,
};

/// `Ya(text, false)` upstream, as `hm` and the WSL flag read a registry or `plutil` answer: nothing for empty text,
/// otherwise `JSON.parse` after a leading byte order mark. `JSON.parse` has no depth limit, and no text nests more
/// arrays and objects than it holds bytes, so a document nested past `serde_json` still reads here and the stage
/// that walks it decides whether the stack holds.
pub(crate) fn parse_policy_answer(text: &str) -> Option<Value> {
    if text.is_empty() {
        return None;
    }
    let text = stand_in_escapes(text.strip_prefix('\u{feff}').unwrap_or(text));
    if nests_past_serde_json(&text) {
        parse_json_deep(&text, text.len())
    } else {
        parse_json(&text).ok()
    }
}

/// Whether a policy source was read (`loadState` upstream).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LoadState {
    Absent,
    Loaded,
    DidNotLoad,
}

impl LoadState {
    /// `qo` upstream: a source that did not load dominates, then one that loaded.
    pub(crate) fn combine(self, other: Self) -> Self {
        if self == Self::DidNotLoad || other == Self::DidNotLoad {
            Self::DidNotLoad
        } else if self == Self::Loaded || other == Self::Loaded {
            Self::Loaded
        } else {
            Self::Absent
        }
    }
}

/// One policy tier as read, without its error messages (they never reach the resolved output).
#[derive(Clone, Debug)]
pub(crate) struct TierRead {
    pub(crate) settings: Option<SettingsMap>,
    pub(crate) has_policy_content: bool,
    pub(crate) load_state: LoadState,
    pub(crate) user_writable: bool,
    /// An error of class `unreadable` was recorded for this tier.
    pub(crate) unreadable: bool,
}

impl TierRead {
    pub(crate) fn absent() -> Self {
        Self {
            settings: None,
            has_policy_content: false,
            load_state: LoadState::Absent,
            user_writable: false,
            unreadable: false,
        }
    }

    pub(crate) fn did_not_load(unreadable: bool) -> Self {
        Self {
            settings: Some(SettingsMap::new()),
            has_policy_content: false,
            load_state: LoadState::DidNotLoad,
            user_writable: false,
            unreadable,
        }
    }

    /// `hm` upstream: parse a policy document handed over as text.
    pub(crate) fn from_hm(text: &str, user_writable: bool) -> Self {
        let Some(Value::Object(mut document)) = parse_policy_answer(text) else {
            let mut tier = Self::did_not_load(false);
            tier.user_writable = user_writable;
            return tier;
        };
        if user_writable {
            for key in MANAGED_ONLY_KEYS
                .iter()
                .chain(["prependPlugins", "appendPlugins"].iter())
            {
                document.shift_remove(*key);
            }
            strip_enabling_plugins(&mut document);
        }
        Self {
            has_policy_content: has_policy_content(&document),
            settings: Some(document),
            load_state: LoadState::Loaded,
            user_writable,
            unreadable: false,
        }
    }

    /// `Uu` upstream.
    pub(crate) fn holds_policy_content(&self) -> bool {
        self.has_policy_content
    }

    /// `lo` upstream.
    pub(crate) fn lo(&self) -> bool {
        !self.user_writable && (self.has_policy_content || self.load_state == LoadState::DidNotLoad)
    }

    /// Whether the tier holds a value that is not an empty object tree (`Nu` upstream).
    pub(crate) fn is_authored(&self) -> bool {
        self.settings.as_ref().is_some_and(has_authored_value)
    }
}

fn strip_enabling_plugins(document: &mut SettingsMap) {
    let Some(Value::Object(plugins)) = document.get("enabledPlugins") else {
        return;
    };
    let enabling = plugins
        .iter()
        .any(|(name, value)| name.ends_with("@builtin") || *value != Value::Bool(false));
    if !enabling {
        return;
    }
    let kept: SettingsMap = plugins
        .iter()
        .filter(|(name, value)| !name.ends_with("@builtin") && **value == Value::Bool(false))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    if kept.is_empty() {
        document.shift_remove("enabledPlugins");
    } else {
        document.insert("enabledPlugins".to_owned(), Value::Object(kept));
    }
}
