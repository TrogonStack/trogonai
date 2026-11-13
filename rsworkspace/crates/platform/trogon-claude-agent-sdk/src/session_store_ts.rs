//! The TypeScript SDK's store-backed session helpers: `listSessions`, `getSessionInfo`, `getSessionMessages`,
//! `listSubagents`, `getSubagentMessages`, `renameSession`, `tagSession`, `forkSession`, `deleteSession` and
//! `importSessionToStore` over a `SessionStore` (bundle functions `Rnt`, `Ont`, `H7`, `vnt`, `wnt`, `knt`, `Pnt`, `gnt`,
//! `hnt`, `xnt`, `mnt` and `V5t`).
//!
//! Entries a store returns are read as the JavaScript object graph the bundle sees: object keys come back in
//! enumeration order, array-index keys first.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use futures_util::future::join_all;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    JsonObject, MessagePage, SdkSessionInfo, SessionEntry, SessionFiles, SessionId, SessionKey, SessionMessage,
    SessionMutationError, SessionProjectKey, SessionStore, SessionStoreOperation, SessionSubpath, SummaryDialect,
    constants::{DEFAULT_IMPORT_BATCH, MAX_IMPORT_BYTES},
    js_object_order::js_ordered,
    session_files::valid_uuid,
    session_files_ts::session_info_from_lite,
    session_fork_ts::{ForkTsRequest, fork_entries, load_state_from_objects},
    session_messages_ts::{session_messages_from_objects, subagent_messages},
    session_mutations::sanitize_unicode_ts,
    session_project_key::{canonicalize_lenient_in, project_name_ts},
    session_prompt::{PromptMode, entry_first_prompt_with, iso_from_epoch_ms, iso_to_epoch_ms, js_space, js_trim},
    session_store_shared::{
        SessionStoreHelperError, StoreListOptions, invalid_session, jsonl_to_lite, main_key, not_found,
    },
    session_summary::summary_entry_to_sdk_info,
};

/// `Wn(directory)` together with the canonical path `CD(directory)` it was derived from.
pub(crate) fn project(directory: Option<&Path>) -> Result<(SessionProjectKey, String), SessionStoreHelperError> {
    let canonical = canonicalize_lenient_in(directory.unwrap_or_else(|| Path::new(".")), SummaryDialect::TypeScript);
    let key = SessionProjectKey::new(project_name_ts(&canonical))?;
    Ok((key, canonical))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// `U7`: one `JSON.stringify` line per entry, each followed by a newline.
fn entries_text(objects: &[JsonObject]) -> String {
    crate::session_fork_ts::serialize_entries(objects)
}

/// A store load as the bundle reads it: `None` for no entries at all.
async fn load_objects(
    store: &(impl SessionStore + ?Sized),
    key: &SessionKey,
) -> Result<Option<Vec<JsonObject>>, SessionStoreHelperError> {
    let entries = store.load(key).await?.unwrap_or_default();
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        entries
            .into_iter()
            .filter_map(|entry| match js_ordered(Value::Object(entry.into_object())) {
                Value::Object(object) => Some(object),
                _ => None,
            })
            .collect(),
    ))
}

/// `trimEnd`.
fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(js_space)
}

/// `Tnt`: the last line's `timestamp`, else the current time.
fn last_timestamp_ms(text: &str) -> i64 {
    let trimmed = js_trim_end(text);
    let last = trimmed.rsplit('\n').next().unwrap_or(trimmed);
    serde_json::from_str::<Value>(last)
        .ok()
        .and_then(|value| {
            value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|stamp| iso_to_epoch_ms(SummaryDialect::TypeScript, stamp))
        })
        .unwrap_or_else(now_ms)
}

/// `F7`: a page of a list, where a limit of zero means no limit.
fn page_of<T>(items: Vec<T>, limit: Option<usize>, offset: usize) -> Vec<T> {
    match limit {
        Some(limit) if limit > 0 => items.into_iter().skip(offset).take(limit).collect(),
        _ => items.into_iter().skip(offset).collect(),
    }
}

/// `H7`: the rows of `slots`, each filled by loading its session. A session that cannot be loaded is a row with an
/// empty summary; one without entries, or one that is a sidechain, has no row.
async fn fill_from_load(
    store: &(impl SessionStore + ?Sized),
    slots: &[(SessionId, u64)],
    project: &SessionProjectKey,
    canonical: &str,
) -> Vec<SdkSessionInfo> {
    let rows = join_all(slots.iter().map(|(id, mtime)| async move {
        let key = SessionKey::new(project.clone(), id.clone());
        let Some(objects) = load_objects(store, &key).await? else {
            return Ok::<Option<SdkSessionInfo>, SessionStoreHelperError>(None);
        };
        let lite = jsonl_to_lite(&entries_text(&objects), *mtime);
        Ok(
            session_info_from_lite(id.as_str(), &lite, Some(canonical), None).map(|mut info| {
                info.last_modified = *mtime;
                info
            }),
        )
    }))
    .await;
    slots
        .iter()
        .zip(rows)
        .filter_map(|((id, mtime), row)| match row {
            Ok(row) => row,
            Err(_) => Some(SdkSessionInfo {
                session_id: id.clone(),
                summary: String::new(),
                last_modified: *mtime,
                file_size: None,
                custom_title: None,
                first_prompt: None,
                git_branch: None,
                cwd: None,
                tag: None,
                created_at: None,
            }),
        })
        .collect()
}

/// A listing slot of `Ont`: `info` is `None` while the row is still to be filled, `Some(None)` for a row known to have
/// no info (a sidechain), which paging still counts.
struct Slot {
    id: SessionId,
    mtime: u64,
    info: Option<Option<SdkSessionInfo>>,
}

/// `Rnt`: sessions of a store, newest first.
pub(crate) async fn list_sessions(
    store: &(impl SessionStore + ?Sized),
    options: &StoreListOptions,
) -> Result<Vec<SdkSessionInfo>, SessionStoreHelperError> {
    let (project, canonical) = project(options.directory.as_deref())?;
    if store.supports(SessionStoreOperation::ListSummaries) {
        return list_from_summaries(store, &project, &canonical, options).await;
    }
    if !store.supports(SessionStoreOperation::List) {
        return Err(SessionStoreHelperError::ListNotImplemented);
    }
    let mut listing: Vec<(SessionId, u64)> = store
        .list_sessions(&project)
        .await?
        .into_iter()
        .map(|entry| (entry.session_id, entry.modified_at_ms))
        .collect();
    listing.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));
    let page = page_of(listing, options.limit, options.offset);
    Ok(fill_from_load(store, &page, &project, &canonical).await)
}

/// `Ont`: the summaries path, gap-filling stale and missing sidecars from `load` when the store can also list.
async fn list_from_summaries(
    store: &(impl SessionStore + ?Sized),
    project: &SessionProjectKey,
    canonical: &str,
    options: &StoreListOptions,
) -> Result<Vec<SdkSessionInfo>, SessionStoreHelperError> {
    let summaries = store.list_session_summaries(project).await?;
    let listing = if store.supports(SessionStoreOperation::List) {
        // A `Map` keyed by session id: the last entry's value, the first entry's position.
        let mut order: Vec<SessionId> = Vec::new();
        let mut latest: HashMap<String, u64> = HashMap::new();
        for entry in store.list_sessions(project).await? {
            if latest
                .insert(entry.session_id.as_str().to_owned(), entry.modified_at_ms)
                .is_none()
            {
                order.push(entry.session_id);
            }
        }
        Some((order, latest))
    } else {
        tracing::warn!(
            "listSessionSummaries without listSessions: gap-fill skipped; sessions lacking a sidecar will be omitted"
        );
        None
    };

    let mut slots: Vec<Slot> = Vec::new();
    for summary in &summaries {
        let known = match &listing {
            Some((_, latest)) => match latest.get(summary.session_id.as_str()) {
                Some(mtime) => Some(*mtime),
                None => continue,
            },
            None => None,
        };
        let stale = known.is_some_and(|mtime| summary.modified_at_ms < mtime);
        slots.push(Slot {
            id: summary.session_id.clone(),
            mtime: if stale {
                known.unwrap_or(summary.modified_at_ms)
            } else {
                summary.modified_at_ms
            },
            info: (!stale).then(|| summary_entry_to_sdk_info(SummaryDialect::TypeScript, summary, Some(canonical))),
        });
    }
    if let Some((order, latest)) = &listing {
        let have: HashSet<&str> = summaries.iter().map(|summary| summary.session_id.as_str()).collect();
        for id in order {
            if !have.contains(id.as_str()) {
                slots.push(Slot {
                    id: id.clone(),
                    mtime: latest.get(id.as_str()).copied().unwrap_or_default(),
                    info: None,
                });
            }
        }
    }
    slots.sort_by_key(|slot| std::cmp::Reverse(slot.mtime));
    let mut page = page_of(slots, options.limit, options.offset);

    let to_fill: Vec<(SessionId, u64)> = page
        .iter()
        .filter(|slot| slot.info.is_none())
        .map(|slot| (slot.id.clone(), slot.mtime))
        .collect();
    if !to_fill.is_empty() {
        let filled: HashMap<String, SdkSessionInfo> = fill_from_load(store, &to_fill, project, canonical)
            .await
            .into_iter()
            .map(|info| (info.session_id.as_str().to_owned(), info))
            .collect();
        for slot in page.iter_mut().filter(|slot| slot.info.is_none()) {
            slot.info = Some(filled.get(slot.id.as_str()).cloned());
        }
    }
    Ok(page.into_iter().filter_map(|slot| slot.info.flatten()).collect())
}

/// `vnt`: one session's info; `None` for an invalid id, no entries or a sidechain.
pub(crate) async fn get_session_info(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
) -> Result<Option<SdkSessionInfo>, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Ok(None);
    }
    let (project, _) = project(directory)?;
    let key = main_key(project, session_id);
    let Some(objects) = load_objects(store, &key).await? else {
        return Ok(None);
    };
    let text = entries_text(&objects);
    let mtime = u64::try_from(last_timestamp_ms(&text)).unwrap_or_default();
    Ok(session_info_from_lite(
        session_id,
        &jsonl_to_lite(&text, mtime),
        None,
        None,
    ))
}

/// `wnt`: a session's visible conversation.
pub(crate) async fn get_session_messages(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
    include_system: bool,
    page: MessagePage,
) -> Result<Vec<SessionMessage>, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Ok(Vec::new());
    }
    let (project, _) = project(directory)?;
    let key = main_key(project, session_id);
    let Some(objects) = load_objects(store, &key).await? else {
        return Ok(Vec::new());
    };
    Ok(session_messages_from_objects(objects, include_system, page)?)
}

/// `knt`: the ids of a session's subagents.
pub(crate) async fn list_subagents(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
) -> Result<Vec<String>, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Ok(Vec::new());
    }
    if !store.supports(SessionStoreOperation::ListSubkeys) {
        return Err(SessionStoreHelperError::ListSubkeysNotImplemented);
    }
    let (project, _) = project(directory)?;
    let key = main_key(project, session_id);
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for subkey in store.list_subkeys(&key).await? {
        if !subkey.starts_with("subagents/") {
            continue;
        }
        let last = subkey.rsplit('/').next().unwrap_or_default();
        if let Some(agent_id) = last.strip_prefix("agent-")
            && seen.insert(agent_id.to_owned())
        {
            ids.push(agent_id.to_owned());
        }
    }
    Ok(ids)
}

/// `Pnt`: a subagent's conversation.
pub(crate) async fn get_subagent_messages(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    agent_id: &str,
    directory: Option<&Path>,
    page: MessagePage,
) -> Result<Vec<SessionMessage>, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() || agent_id.is_empty() {
        return Ok(Vec::new());
    }
    let (project, _) = project(directory)?;
    let base = main_key(project, session_id);
    let mut subpath = format!("subagents/agent-{agent_id}");
    if store.supports(SessionStoreOperation::ListSubkeys) {
        let target = format!("agent-{agent_id}");
        let found = store
            .list_subkeys(&base)
            .await?
            .into_iter()
            .find(|subkey| subkey.starts_with("subagents/") && subkey.rsplit('/').next() == Some(target.as_str()));
        match found {
            Some(found) => subpath = found,
            None => return Ok(Vec::new()),
        }
    }
    let Some(objects) = load_objects(store, &base.with_subpath(SessionSubpath::new(subpath))).await? else {
        return Ok(Vec::new());
    };
    let is_metadata = |object: &JsonObject| object.get("type").and_then(Value::as_str) == Some("agent_metadata");
    let metadata = objects.iter().rfind(|object| is_metadata(object));
    let field = |name: &str| {
        metadata
            .and_then(|object| object.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let (tool_use_id, parent_agent_id) = (field("toolUseId"), field("parentAgentId"));
    let transcript: Vec<JsonObject> = objects.into_iter().filter(|object| !is_metadata(object)).collect();
    if transcript.is_empty() {
        return Ok(Vec::new());
    }
    Ok(subagent_messages(
        entries_text(&transcript).as_bytes(),
        page,
        tool_use_id.as_deref(),
        parent_agent_id.as_deref(),
    ))
}

fn metadata_entry(fields: [(&str, Value); 3]) -> SessionEntry {
    let mut object = JsonObject::new();
    for (key, value) in fields {
        object.insert(key.to_owned(), value);
    }
    object.insert("uuid".into(), Value::from(uuid::Uuid::new_v4().to_string()));
    object.insert("timestamp".into(), Value::from(iso_from_epoch_ms(now_ms())));
    SessionEntry::new(object)
}

/// `gnt`: renames a session by appending a `custom-title` entry.
pub(crate) async fn rename_session(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    title: &str,
    directory: Option<&Path>,
) -> Result<(), SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Err(invalid_session(session_id));
    }
    let title = js_trim(title);
    if title.is_empty() {
        return Err(SessionMutationError::EmptyTitle.into());
    }
    let (project, _) = project(directory)?;
    let key = main_key(project, session_id);
    let entry = metadata_entry([
        ("type", Value::from("custom-title")),
        ("customTitle", Value::from(title)),
        ("sessionId", Value::from(session_id)),
    ]);
    store.append(&key, &[entry]).await?;
    Ok(())
}

/// `hnt`: tags a session by appending a `tag` entry; `None` clears the tag.
pub(crate) async fn tag_session(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    tag: Option<&str>,
    directory: Option<&Path>,
) -> Result<(), SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Err(invalid_session(session_id));
    }
    let tag = match tag {
        Some(tag) => {
            let sanitized = sanitize_unicode_ts(tag)?;
            let trimmed = js_trim(&sanitized);
            if trimmed.is_empty() {
                return Err(SessionMutationError::EmptyTag.into());
            }
            trimmed.to_owned()
        }
        None => String::new(),
    };
    let (project, _) = project(directory)?;
    let key = main_key(project, session_id);
    let entry = metadata_entry([
        ("type", Value::from("tag")),
        ("tag", Value::from(tag)),
        ("sessionId", Value::from(session_id)),
    ]);
    store.append(&key, &[entry]).await?;
    Ok(())
}

/// `mnt`: deletes a session; a store without `delete` is left alone.
pub(crate) async fn delete_session(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
) -> Result<(), SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Err(invalid_session(session_id));
    }
    if !store.supports(SessionStoreOperation::Delete) {
        return Ok(());
    }
    let (project, _) = project(directory)?;
    store.delete(&main_key(project, session_id)).await?;
    Ok(())
}

/// `BMe`: the last custom title, else the last AI title, else the first prompt as it is shown.
fn derived_title(objects: &[JsonObject]) -> String {
    let (mut custom, mut ai) = (None, None);
    for object in objects {
        if let Some(title) = object
            .get("customTitle")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
        {
            custom = Some(title);
        }
        if let Some(title) = object
            .get("aiTitle")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
        {
            ai = Some(title);
        }
    }
    if let Some(title) = custom.or(ai) {
        return title.to_owned();
    }
    let mut fallback = Some(String::new());
    for object in objects {
        if let Some(prompt) =
            entry_first_prompt_with(SummaryDialect::TypeScript, PromptMode::Shown, object, &mut fallback)
        {
            return prompt;
        }
    }
    fallback.unwrap_or_default()
}

/// `xnt`: forks a stored session under the same project key.
pub(crate) async fn fork_session(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
    up_to_message_id: Option<&str>,
    title: Option<&str>,
) -> Result<crate::ForkSessionResult, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Err(invalid_session(session_id));
    }
    let (project, _) = project(directory)?;
    let source = main_key(project.clone(), session_id);
    let Some(objects) = load_objects(store, &source).await? else {
        return Err(not_found(session_id));
    };
    let state = load_state_from_objects(&objects, session_id);
    let mut ids = || uuid::Uuid::new_v4().to_string();
    let mut clock = now_ms;
    let forked = fork_entries(
        &state,
        &ForkTsRequest {
            session_id,
            up_to_message_id,
            title,
        },
        || derived_title(&objects),
        &mut ids,
        &mut clock,
    )
    .map_err(SessionMutationError::from)?;
    let entries: Vec<SessionEntry> = forked.entries.into_iter().map(SessionEntry::new).collect();
    store.append(&main_key(project, &forked.session_id), &entries).await?;
    Ok(crate::ForkSessionResult {
        session_id: SessionId::from(forked.session_id),
    })
}

/// `V5t`: replays a local transcript, and optionally its subagent transcripts and sidecars, into a store under the key
/// derived from the directory, as a session mirrored from that directory would be stored.
pub(crate) async fn import_session(
    files: &SessionFiles,
    session_id: &str,
    store: &(impl SessionStore + ?Sized),
    options: &crate::ImportOptions,
) -> Result<(), SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Err(invalid_session(session_id));
    }
    let resolved = files
        .resolve_session_file_path(session_id, options.directory.as_deref())
        .ok_or_else(|| not_found(session_id))?;
    let (project, _) = project(options.directory.as_deref())?;
    let batch_size = if options.batch_size == 0 {
        DEFAULT_IMPORT_BATCH
    } else {
        options.batch_size
    };
    let main = main_key(project, session_id);
    append_jsonl_in_batches(&resolved, &main, store, batch_size).await?;
    if !options.include_subagents {
        return Ok(());
    }
    let session_dir = resolved.with_extension("");
    for file in collect_jsonl_files(&session_dir.join("subagents")) {
        let Ok(relative) = file.strip_prefix(&session_dir) else {
            continue;
        };
        let mut parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        if let Some(last) = parts.last_mut() {
            last.truncate(last.len().saturating_sub(".jsonl".len()));
        }
        let subpath = SessionSubpath::new(parts.join("/"));
        let key = main.clone().with_subpath(subpath);
        append_jsonl_in_batches(&file, &key, store, batch_size).await?;
        let sidecar = file.with_extension("meta.json");
        let metadata = match tokio::fs::read(&sidecar).await {
            Ok(bytes) => match serde_json::from_str::<Value>(&String::from_utf8_lossy(&bytes)) {
                Ok(value) => Some(value),
                Err(error) => {
                    tracing::warn!(
                        path = %sidecar.display(),
                        %error,
                        "importSessionToStore: skipping unreadable sidecar"
                    );
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                tracing::warn!(
                    path = %sidecar.display(),
                    %error,
                    "importSessionToStore: skipping unreadable sidecar"
                );
                None
            }
        };
        if let Some(metadata) = metadata {
            store.append(&key, &[metadata_sidecar_entry(metadata)]).await?;
        }
    }
    Ok(())
}

/// `{type: "agent_metadata", ...sidecar}`: the sidecar's own fields follow `type` and win over it, and spreading a
/// non-object contributes its own enumerable properties (none for a number, boolean or null).
fn metadata_sidecar_entry(sidecar: Value) -> SessionEntry {
    let mut object = JsonObject::new();
    object.insert("type".into(), Value::from("agent_metadata"));
    let spread = js_ordered(sidecar);
    match spread {
        Value::Object(fields) => object.extend(fields),
        Value::Array(items) => {
            for (index, item) in items.into_iter().enumerate() {
                object.insert(index.to_string(), item);
            }
        }
        Value::String(text) => {
            for (index, unit) in text.encode_utf16().enumerate() {
                let ch = String::from_utf16_lossy(&[unit]);
                object.insert(index.to_string(), Value::from(ch));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    SessionEntry::new(js_reordered(object))
}

fn js_reordered(object: JsonObject) -> JsonObject {
    match js_ordered(Value::Object(object)) {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    }
}

/// `M7`: reads a transcript line by line into entries, skipping empty and unparseable lines, and appends them in
/// batches bounded by an entry count and by the UTF-16 length of the lines.
async fn append_jsonl_in_batches(
    path: &Path,
    key: &SessionKey,
    store: &(impl SessionStore + ?Sized),
    batch_size: usize,
) -> Result<(), SessionStoreHelperError> {
    let file = tokio::fs::File::open(path).await.map_err(SessionStoreHelperError::Io)?;
    let mut reader = BufReader::new(file);
    let mut batch: Vec<SessionEntry> = Vec::new();
    let mut units = 0_usize;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if reader
            .read_until(b'\n', &mut raw)
            .await
            .map_err(SessionStoreHelperError::Io)?
            == 0
        {
            break;
        }
        if raw.last() == Some(&b'\n') {
            raw.pop();
        }
        if raw.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(&raw);
        match serde_json::from_str::<Value>(&line) {
            Ok(Value::Object(object)) => batch.push(SessionEntry::new(js_reordered(object))),
            Ok(_) => {
                tracing::warn!(path = %path.display(), "importSessionToStore: skipping non-object line");
                continue;
            }
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "importSessionToStore: skipping unparseable line"
                );
                continue;
            }
        }
        units += line.encode_utf16().count();
        if batch.len() >= batch_size || units >= MAX_IMPORT_BYTES {
            store.append(key, &batch).await?;
            batch.clear();
            units = 0;
        }
    }
    if !batch.is_empty() {
        store.append(key, &batch).await?;
    }
    Ok(())
}

/// `Ant`: every regular `*.jsonl` file below `base`, directories followed only when they are not symbolic links.
fn collect_jsonl_files(base: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, PathBuf, std::fs::FileType)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            Some((entry.file_name().to_string_lossy().into_owned(), entry.path(), kind))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut files = Vec::new();
    for (name, path, kind) in entries {
        if kind.is_dir() {
            files.extend(collect_jsonl_files(&path));
        } else if kind.is_file() && name.ends_with(".jsonl") {
            files.push(path);
        }
    }
    files
}

#[cfg(test)]
#[path = "session_store_oracle_tests.rs"]
mod oracle_tests;
