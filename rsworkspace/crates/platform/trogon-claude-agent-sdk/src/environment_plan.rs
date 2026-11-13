//! The child environment as a pure plan, so tests can pin it without spawning.
//!
//! `Overlay` follows Python `subprocess_cli.py` (0.2.181) and `Replace`/`Inherit` follow `sdk.mjs` (0.3.323).

use crate::constants::{DEFAULT_ENTRYPOINT, SDK_VERSION};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
};

use crate::{
    ClaudeAgentOptions, FileCheckpointing,
    process_options::{ClaudeCodeMarker, EnvironmentMode},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EnvironmentPlan {
    /// Start from an empty environment instead of the inherited one.
    pub clear: bool,
    pub set: BTreeMap<OsString, OsString>,
    pub remove: BTreeSet<OsString>,
}

pub(crate) type Inherited = BTreeMap<OsString, OsString>;

fn truthy(value: Option<&OsString>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.to_string_lossy().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) fn plan(options: &ClaudeAgentOptions, inherited: &Inherited) -> EnvironmentPlan {
    let configured: BTreeMap<OsString, OsString> = options
        .env
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value.expose())))
        .collect();
    let configured_has = |name: &str| configured.keys().any(|key| key == OsStr::new(name));
    let configured_non_empty = |name: &str| configured.get(OsStr::new(name)).is_some_and(|value| !value.is_empty());
    let entrypoint = options.entrypoint.as_deref().unwrap_or(DEFAULT_ENTRYPOINT);
    let mut plan = EnvironmentPlan::default();
    let mode = options.environment_mode;

    let remove_marker = match options.claude_code_marker {
        ClaudeCodeMarker::Remove => true,
        ClaudeCodeMarker::Keep => false,
        ClaudeCodeMarker::ModeDefault => mode == EnvironmentMode::Overlay,
    };
    if remove_marker {
        plan.remove.insert("CLAUDECODE".into());
    }

    match mode {
        EnvironmentMode::Overlay => {
            plan.set.insert("CLAUDE_CODE_ENTRYPOINT".into(), entrypoint.into());
            plan.set.extend(configured.clone());
            plan.set.insert("CLAUDE_AGENT_SDK_VERSION".into(), SDK_VERSION.into());
            plan.remove.insert("CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP".into());
            if let Some(app) = &options.client_app {
                plan.set.insert("CLAUDE_AGENT_SDK_CLIENT_APP".into(), app.into());
            }
            if let Some(cwd) = &options.cwd {
                plan.set.insert("PWD".into(), cwd.as_os_str().to_owned());
            }
        }
        EnvironmentMode::Replace => {
            plan.clear = true;
            plan.set.extend(configured.clone());
            plan.set.remove(OsStr::new("NODE_OPTIONS"));
            plan.set.remove(OsStr::new("DEBUG"));
            if !configured_non_empty("CLAUDE_CODE_ENTRYPOINT") {
                plan.set.insert("CLAUDE_CODE_ENTRYPOINT".into(), entrypoint.into());
            }
            if !configured_non_empty("CLAUDE_AGENT_SDK_VERSION") {
                plan.set.insert("CLAUDE_AGENT_SDK_VERSION".into(), SDK_VERSION.into());
            }
            if truthy(inherited.get(OsStr::new("DEBUG_CLAUDE_AGENT_SDK"))) {
                plan.set.insert("DEBUG".into(), "1".into());
            }
        }
        EnvironmentMode::Inherit => {
            if !inherited.contains_key(OsStr::new("CLAUDE_CODE_ENTRYPOINT")) {
                plan.set.insert("CLAUDE_CODE_ENTRYPOINT".into(), entrypoint.into());
            }
            plan.set.insert("CLAUDE_AGENT_SDK_VERSION".into(), SDK_VERSION.into());
            plan.remove.insert("NODE_OPTIONS".into());
            if truthy(inherited.get(OsStr::new("DEBUG_CLAUDE_AGENT_SDK"))) {
                plan.set.insert("DEBUG".into(), "1".into());
            } else {
                plan.remove.insert("DEBUG".into());
            }
        }
    }

    for (name, value) in [
        (
            "TRACEPARENT",
            options.trace_context.as_ref().map(|t| t.traceparent.as_str()),
        ),
        (
            "TRACESTATE",
            options.trace_context.as_ref().and_then(|t| t.tracestate.as_deref()),
        ),
    ] {
        if configured_has(name) {
            continue;
        }
        plan.remove.insert(name.into());
        if let Some(value) = value {
            plan.remove.remove(OsStr::new(name));
            plan.set.insert(name.into(), value.into());
        }
    }

    if options.file_checkpointing == FileCheckpointing::Enabled {
        plan.set
            .insert("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING".into(), "true".into());
    }

    let reads = "CLAUDE_CODE_SDK_READS_SESSION_STATE";
    // TypeScript's replace mode starts from `options.env` alone; the other modes see the inherited environment too.
    let present = configured
        .keys()
        .any(|key| key.to_string_lossy().eq_ignore_ascii_case(reads))
        || (mode != EnvironmentMode::Replace
            && inherited
                .keys()
                .any(|key| key.to_string_lossy().eq_ignore_ascii_case(reads)));
    if !present {
        plan.set.insert(reads.into(), "1".into());
    }
    plan
}

#[cfg(test)]
mod tests;
