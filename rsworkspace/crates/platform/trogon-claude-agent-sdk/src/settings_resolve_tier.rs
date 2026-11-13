use serde_json::Value;

use crate::{
    constants::MANAGED_ONLY_KEYS,
    settings_resolve_values::{SettingsMap, has_policy_content},
};

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
        let parsed = (!text.is_empty())
            .then(|| serde_json::from_str::<Value>(text).ok())
            .flatten();
        let Some(Value::Object(mut document)) = parsed else {
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
        self.settings
            .as_ref()
            .is_some_and(crate::settings_resolve_values::has_authored_value)
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
