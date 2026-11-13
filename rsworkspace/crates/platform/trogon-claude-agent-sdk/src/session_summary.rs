//! Incremental session summary folding for [`SessionStore`](crate::SessionStore) adapters.
//!
//! Upstream `foldSessionSummary` / `fold_session_summary` keep a per-session sidecar current inside `append()` so a
//! listing needs one `list_session_summaries()` call instead of one `load()` per session. Both SDKs persist the
//! opaque `data` blob with different key casing, so a store picks the [`SummaryDialect`] it folds.

use serde_json::Value;

use crate::{
    JsonObject, SdkSessionInfo, SessionEntry, SessionId, SessionKey, SummaryDialect,
    session_prompt::{entry_first_prompt, iso_to_epoch_ms},
};

/// Per-session summary sidecar maintained by [`fold_session_summary`].
///
/// `data` is opaque to stores: persist it verbatim. `modified_at_ms` is the storage write time and must share a clock
/// with [`SessionListEntry::modified_at_ms`](crate::SessionListEntry); the fold never derives it from entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSummaryEntry {
    pub session_id: SessionId,
    pub modified_at_ms: u64,
    pub data: JsonObject,
}

struct Keys {
    is_sidechain: &'static str,
    created_at: &'static str,
    first_prompt: &'static str,
    first_prompt_locked: &'static str,
    command_fallback: &'static str,
    custom_title: &'static str,
    ai_title: &'static str,
    last_prompt: &'static str,
    summary_hint: &'static str,
    git_branch: &'static str,
}

impl SummaryDialect {
    fn keys(self) -> Keys {
        match self {
            Self::TypeScript => Keys {
                is_sidechain: "isSidechain",
                created_at: "createdAt",
                first_prompt: "firstPrompt",
                first_prompt_locked: "firstPromptLocked",
                command_fallback: "commandFallback",
                custom_title: "customTitle",
                ai_title: "aiTitle",
                last_prompt: "lastPrompt",
                summary_hint: "summaryHint",
                git_branch: "gitBranch",
            },
            Self::Python => Keys {
                is_sidechain: "is_sidechain",
                created_at: "created_at",
                first_prompt: "first_prompt",
                first_prompt_locked: "first_prompt_locked",
                command_fallback: "command_fallback",
                custom_title: "custom_title",
                ai_title: "ai_title",
                last_prompt: "last_prompt",
                summary_hint: "summary_hint",
                git_branch: "git_branch",
            },
        }
    }
}

/// Folds a batch of appended entries into the running summary for `key`.
///
/// Call this from `append()` for main transcripts only; subagent keys must not contribute. `mtime` stamps the
/// sidecar write time; when absent the previous summary's time is kept, or `0` for a first fold.
pub fn fold_session_summary(
    dialect: SummaryDialect,
    prev: Option<&SessionSummaryEntry>,
    key: &SessionKey,
    entries: &[SessionEntry],
    mtime: Option<u64>,
) -> SessionSummaryEntry {
    let keys = dialect.keys();
    let mut summary = match prev {
        Some(prev) => prev.clone(),
        None => SessionSummaryEntry {
            session_id: key.session_id().clone(),
            modified_at_ms: 0,
            data: JsonObject::new(),
        },
    };
    if let Some(mtime) = mtime {
        summary.modified_at_ms = mtime;
    }
    let data = &mut summary.data;
    for entry in entries {
        let entry = entry.as_object();
        let created = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|text| iso_to_epoch_ms(dialect, text));
        if !data.contains_key(keys.is_sidechain) {
            data.insert(
                keys.is_sidechain.to_owned(),
                Value::Bool(entry.get("isSidechain").and_then(Value::as_bool) == Some(true)),
            );
        }
        if !data.contains_key(keys.created_at)
            && let Some(created) = created
        {
            data.insert(keys.created_at.to_owned(), Value::from(created));
        }
        if !data.contains_key("cwd")
            && let Some(cwd) = entry.get("cwd").and_then(Value::as_str).filter(|cwd| !cwd.is_empty())
        {
            data.insert("cwd".to_owned(), Value::String(cwd.to_owned()));
        }
        fold_first_prompt(dialect, &keys, data, entry);
        for (source, target) in [
            ("customTitle", keys.custom_title),
            ("aiTitle", keys.ai_title),
            ("lastPrompt", keys.last_prompt),
            ("summary", keys.summary_hint),
            ("gitBranch", keys.git_branch),
        ] {
            if let Some(value) = entry.get(source).and_then(Value::as_str) {
                data.insert(target.to_owned(), Value::String(value.to_owned()));
            }
        }
        if entry.get("type").and_then(Value::as_str) == Some("tag") {
            match entry.get("tag").and_then(Value::as_str).filter(|tag| !tag.is_empty()) {
                Some(tag) => {
                    data.insert("tag".to_owned(), Value::String(tag.to_owned()));
                }
                None => {
                    data.remove("tag");
                }
            }
        }
        if dialect == SummaryDialect::TypeScript
            && entry.get("type").and_then(Value::as_str) == Some("relocated")
            && let Some(cwd) = entry
                .get("relocatedCwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.is_empty())
        {
            data.insert("cwd".to_owned(), Value::String(cwd.to_owned()));
        }
    }
    summary
}

fn fold_first_prompt(dialect: SummaryDialect, keys: &Keys, data: &mut JsonObject, entry: &JsonObject) {
    if data.get(keys.first_prompt_locked).is_some_and(truthy) {
        return;
    }
    let mut fallback = data
        .get(keys.command_fallback)
        .and_then(Value::as_str)
        .map(str::to_owned);
    let prompt = entry_first_prompt(dialect, entry, &mut fallback);
    if let Some(fallback) = fallback.filter(|text| !text.is_empty())
        && data
            .get(keys.command_fallback)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        data.insert(keys.command_fallback.to_owned(), Value::String(fallback));
    }
    if let Some(prompt) = prompt {
        data.insert(keys.first_prompt.to_owned(), Value::String(prompt));
        data.insert(keys.first_prompt_locked.to_owned(), Value::Bool(true));
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Converts a summary sidecar into the listing record.
///
/// Returns `None` for sidechain sessions and sessions without a displayable summary, matching the filesystem lite
/// parse. `project_path` is the fallback working directory.
pub fn summary_entry_to_sdk_info(
    dialect: SummaryDialect,
    entry: &SessionSummaryEntry,
    project_path: Option<&str>,
) -> Option<SdkSessionInfo> {
    let keys = dialect.keys();
    let data = &entry.data;
    if data.get(keys.is_sidechain).is_some_and(truthy) {
        return None;
    }
    let text = |name: &str| data.get(name).and_then(Value::as_str).filter(|text| !text.is_empty());
    let first_prompt = if data.get(keys.first_prompt_locked).is_some_and(truthy) {
        text(keys.first_prompt)
    } else {
        text(keys.command_fallback)
    };
    let custom_title = text(keys.custom_title).or_else(|| text(keys.ai_title));
    let summary = custom_title
        .or_else(|| text(keys.last_prompt))
        .or_else(|| text(keys.summary_hint))
        .or(first_prompt)?;
    Some(SdkSessionInfo {
        session_id: entry.session_id.clone(),
        summary: summary.to_owned(),
        last_modified: entry.modified_at_ms,
        file_size: None,
        custom_title: custom_title.map(str::to_owned),
        first_prompt: first_prompt.map(str::to_owned),
        git_branch: text(keys.git_branch).map(str::to_owned),
        cwd: text("cwd")
            .or(project_path.filter(|path| !path.is_empty()))
            .map(str::to_owned),
        tag: text("tag").map(str::to_owned),
        created_at: data.get(keys.created_at).and_then(Value::as_i64),
    })
}
