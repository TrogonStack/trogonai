//! Store-backed session helpers and the filesystem-to-store import.
//!
//! Ports upstream `list_sessions_from_store`, `get_session_info_from_store`, `get_session_messages_from_store`,
//! `list_subagents_from_store`, `get_subagent_messages_from_store`, the `*_via_store` mutations and
//! `import_session_to_store`. Project keys use the TypeScript sanitization the transcript mirror uses, so they match
//! what a mirrored or imported session was stored under.

use crate::constants::{DEFAULT_IMPORT_BATCH, LIST_LOAD_CONCURRENCY, LITE_READ_BUF_SIZE, MAX_IMPORT_BYTES};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use futures_util::{StreamExt, stream};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    JsonObject, MessagePage, SdkSessionInfo, SessionEntry, SessionFiles, SessionId, SessionKey, SessionKeyError,
    SessionMessage, SessionMutationError, SessionProjectKey, SessionProjectKeyError, SessionStore, SessionStoreError,
    SessionStoreOperation, SessionSubpath, SummaryDialect, project_key_for_directory,
    session_files::{
        LiteSessionFile, extract_first_prompt_from_head, parse_session_info_from_lite, read_agent_metadata_sidecar,
        sort_page, valid_uuid,
    },
    session_mutations::{build_fork_lines, iso_now, partition_fork_entry, py_strip, sanitize_unicode},
    session_project_key::canonicalize_lenient_in,
    session_prompt::iso_to_epoch_ms,
    session_summary::summary_entry_to_sdk_info,
    session_transcript::{
        entries_to_session_messages, entries_to_subagent_messages, is_transcript_entry, parent_ids_from_agent_metadata,
    },
};

#[derive(Debug, thiserror::Error)]
pub enum SessionStoreHelperError {
    #[error(transparent)]
    Mutation(#[from] SessionMutationError),
    #[error(transparent)]
    Store(#[from] SessionStoreError),
    #[error(transparent)]
    Project(#[from] SessionProjectKeyError),
    #[error(transparent)]
    Key(#[from] SessionKeyError),
    #[error("session store implements no {0}; it cannot serve this helper")]
    UnsupportedStore(&'static str),
    #[error("session file I/O failed")]
    Io(#[source] std::io::Error),
    #[error("session transcript line is not valid JSON")]
    Encoding(#[source] serde_json::Error),
    #[error("session transcript line is not a JSON object")]
    NotObject,
}

/// Arguments of [`list_sessions_from_store`].
#[derive(Clone, Debug, Default)]
pub struct StoreListOptions {
    /// Project directory the key is derived from; the current directory when empty.
    pub directory: Option<PathBuf>,
    pub limit: Option<usize>,
    pub offset: usize,
    /// How the store's summary sidecars were folded; match the dialect the store was built with.
    pub summary_dialect: SummaryDialect,
}

/// Arguments of [`import_session_to_store`].
#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub directory: Option<PathBuf>,
    pub include_subagents: bool,
    /// Entries per `append`; zero means the default of 500.
    pub batch_size: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            directory: None,
            include_subagents: true,
            batch_size: DEFAULT_IMPORT_BATCH,
        }
    }
}

fn project_key(directory: Option<&Path>) -> Result<SessionProjectKey, SessionStoreHelperError> {
    Ok(project_key_for_directory(
        directory.filter(|d| !d.as_os_str().is_empty()),
    )?)
}

fn project_path(directory: Option<&Path>) -> String {
    canonicalize_lenient_in(
        directory
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
        crate::SummaryDialect::TypeScript,
    )
}

fn main_key(project: SessionProjectKey, session_id: &str) -> Result<SessionKey, SessionStoreHelperError> {
    Ok(SessionKey::new(project, SessionId::from(session_id))?)
}

fn invalid_session(session_id: &str) -> SessionStoreHelperError {
    SessionMutationError::InvalidSessionId(session_id.to_owned()).into()
}

fn not_found(session_id: &str) -> SessionStoreHelperError {
    SessionMutationError::NotFound {
        session_id: session_id.to_owned(),
        directory: None,
    }
    .into()
}

/// Upstream `_entries_to_jsonl`: one compact line per entry with `type` hoisted first, because the lite parse scans
/// for the literal prefix `{"type":"tag"`.
fn entries_to_jsonl(entries: &[SessionEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        let object = entry.as_object();
        let mut fields: Vec<String> = Vec::with_capacity(object.len());
        if let Some(kind) = object.get("type") {
            fields.push(format!("\"type\":{kind}"));
        }
        for (key, value) in object {
            if key != "type" {
                fields.push(format!("{}:{value}", Value::from(key.as_str())));
            }
        }
        out.push('{');
        out.push_str(&fields.join(","));
        out.push_str("}\n");
    }
    out
}

/// Upstream `_jsonl_to_lite`: the same head and tail slices the disk path would read.
fn jsonl_to_lite(jsonl: &str, mtime: u64) -> LiteSessionFile {
    let bytes = jsonl.as_bytes();
    let size = bytes.len();
    let window = LITE_READ_BUF_SIZE as usize;
    let head = String::from_utf8_lossy(&bytes[..size.min(window)]).into_owned();
    let tail = if size > window {
        String::from_utf8_lossy(&bytes[size - window..]).into_owned()
    } else {
        head.clone()
    };
    LiteSessionFile {
        mtime,
        size: size as u64,
        head,
        tail,
    }
}

/// Upstream `_mtime_from_jsonl_tail`: the last entry's timestamp, else the current time.
fn mtime_from_jsonl_tail(jsonl: &str) -> u64 {
    let trimmed = jsonl.trim_end();
    let last = trimmed.rsplit('\n').next().unwrap_or(trimmed);
    let parsed = serde_json::from_str::<Value>(last)
        .ok()
        .and_then(|value| value.get("timestamp").and_then(Value::as_str).map(str::to_owned))
        .and_then(|stamp| iso_to_epoch_ms(SummaryDialect::Python, &stamp))
        .and_then(|ms| u64::try_from(ms).ok());
    parsed.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
    })
}

async fn load_entries(
    store: &(impl SessionStore + ?Sized),
    key: &SessionKey,
) -> Result<Vec<SessionEntry>, SessionStoreHelperError> {
    Ok(store.load(key).await?.unwrap_or_default())
}

fn filter_transcript(entries: Vec<SessionEntry>) -> Vec<JsonObject> {
    entries
        .into_iter()
        .map(SessionEntry::into_object)
        .filter(is_transcript_entry)
        .collect()
}

/// Upstream `_derive_infos_via_load`: bounded concurrent loads; a failed load degrades its row to an empty summary,
/// and sidechain or summary-less sessions are dropped.
async fn derive_infos_via_load(
    store: &(impl SessionStore + ?Sized),
    listing: &[(SessionId, u64)],
    project: &SessionProjectKey,
    project_path: &str,
) -> Vec<SdkSessionInfo> {
    let keys: Vec<Option<SessionKey>> = listing
        .iter()
        .map(|(id, _)| SessionKey::new(project.clone(), id.clone()).ok())
        .collect();
    let outcomes: Vec<Option<Result<Option<Vec<SessionEntry>>, SessionStoreError>>> = stream::iter(keys.iter())
        .map(|key| async move {
            match key {
                Some(key) => Some(store.load(key).await),
                None => None,
            }
        })
        .buffered(LIST_LOAD_CONCURRENCY)
        .collect()
        .await;

    let mut results = Vec::new();
    for ((id, mtime), outcome) in listing.iter().zip(outcomes) {
        match outcome {
            Some(Ok(Some(entries))) if !entries.is_empty() => {
                let lite = jsonl_to_lite(&entries_to_jsonl(&entries), *mtime);
                if let Some(mut info) = parse_session_info_from_lite(id.as_str(), &lite, Some(project_path)) {
                    info.last_modified = *mtime;
                    results.push(info);
                }
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => results.push(SdkSessionInfo {
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
        }
    }
    results
}

enum Slot {
    Ready(u64, SdkSessionInfo),
    Fill(u64, SessionId),
}

impl Slot {
    fn mtime(&self) -> u64 {
        match self {
            Self::Ready(mtime, _) | Self::Fill(mtime, _) => *mtime,
        }
    }
}

/// Lists sessions from a store, newest first. Uses the store's summary sidecars when it has them, gap-filling stale
/// or missing ones from `load`, and otherwise loads each session (sixteen at a time).
pub async fn list_sessions_from_store(
    store: &(impl SessionStore + ?Sized),
    options: &StoreListOptions,
) -> Result<Vec<SdkSessionInfo>, SessionStoreHelperError> {
    let directory = options.directory.as_deref();
    let path = project_path(directory);
    let project = project_key(directory)?;
    let has_list = store.supports(SessionStoreOperation::List);

    if store.supports(SessionStoreOperation::ListSummaries) {
        match store.list_session_summaries(&project).await {
            Ok(summaries) => {
                let listing: Vec<(SessionId, u64)> = if has_list {
                    store
                        .list_sessions(&project)
                        .await?
                        .into_iter()
                        .map(|entry| (entry.session_id, entry.modified_at_ms))
                        .collect()
                } else {
                    Vec::new()
                };
                let known: HashMap<&str, u64> = listing.iter().map(|(id, mtime)| (id.as_str(), *mtime)).collect();

                let mut slots: Vec<Slot> = Vec::new();
                let mut fresh: HashSet<String> = HashSet::new();
                for summary in &summaries {
                    if has_list {
                        match known.get(summary.session_id.as_str()) {
                            None => continue,
                            Some(known_mtime) if summary.modified_at_ms < *known_mtime => continue,
                            Some(_) => {}
                        }
                    }
                    fresh.insert(summary.session_id.as_str().to_owned());
                    if let Some(info) = summary_entry_to_sdk_info(options.summary_dialect, summary, Some(&path)) {
                        slots.push(Slot::Ready(summary.modified_at_ms, info));
                    }
                }
                if has_list {
                    slots.extend(
                        listing
                            .iter()
                            .filter(|(id, _)| !fresh.contains(id.as_str()))
                            .map(|(id, mtime)| Slot::Fill(*mtime, id.clone())),
                    );
                }

                slots.sort_by_key(|slot| std::cmp::Reverse(slot.mtime()));
                let page: Vec<Slot> = if options.offset > 0 {
                    slots.into_iter().skip(options.offset).collect()
                } else {
                    slots
                };
                let page: Vec<Slot> = match options.limit {
                    Some(limit) if limit > 0 => page.into_iter().take(limit).collect(),
                    _ => page,
                };

                let to_fill: Vec<(SessionId, u64)> = page
                    .iter()
                    .filter_map(|slot| match slot {
                        Slot::Fill(mtime, id) => Some((id.clone(), *mtime)),
                        Slot::Ready(..) => None,
                    })
                    .collect();
                let mut filled: HashMap<String, SdkSessionInfo> = if to_fill.is_empty() {
                    HashMap::new()
                } else {
                    derive_infos_via_load(store, &to_fill, &project, &path)
                        .await
                        .into_iter()
                        .map(|info| (info.session_id.as_str().to_owned(), info))
                        .collect()
                };
                return Ok(page
                    .into_iter()
                    .filter_map(|slot| match slot {
                        Slot::Ready(_, info) => Some(info),
                        Slot::Fill(_, id) => filled.remove(id.as_str()),
                    })
                    .collect());
            }
            Err(SessionStoreError::Unsupported(_)) => {}
            Err(error) => return Err(error.into()),
        }
    }

    if !has_list {
        return Err(SessionStoreHelperError::UnsupportedStore(
            "list_session_summaries or list_sessions",
        ));
    }
    let listing: Vec<(SessionId, u64)> = store
        .list_sessions(&project)
        .await?
        .into_iter()
        .map(|entry| (entry.session_id, entry.modified_at_ms))
        .collect();
    let infos = derive_infos_via_load(store, &listing, &project, &path).await;
    Ok(sort_page(infos, options.limit, options.offset))
}

/// Reads one session's metadata from a store; `None` for an invalid id, a missing session, a sidechain or a session
/// without a summary.
pub async fn get_session_info_from_store(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
) -> Result<Option<SdkSessionInfo>, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Ok(None);
    }
    let key = main_key(project_key(directory)?, session_id)?;
    let entries = load_entries(store, &key).await?;
    if entries.is_empty() {
        return Ok(None);
    }
    let jsonl = entries_to_jsonl(&entries);
    let lite = jsonl_to_lite(&jsonl, mtime_from_jsonl_tail(&jsonl));
    Ok(parse_session_info_from_lite(
        session_id,
        &lite,
        Some(&project_path(directory)),
    ))
}

/// Reads a session's conversation from a store; empty for an invalid id or a missing session.
pub async fn get_session_messages_from_store(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
    page: MessagePage,
) -> Result<Vec<SessionMessage>, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Ok(Vec::new());
    }
    let key = main_key(project_key(directory)?, session_id)?;
    let entries = load_entries(store, &key).await?;
    Ok(entries_to_session_messages(&filter_transcript(entries), page))
}

/// Lists subagent ids from the store's subkeys; the store must implement `list_subkeys`.
pub async fn list_subagents_from_store(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
) -> Result<Vec<String>, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Ok(Vec::new());
    }
    if !store.supports(SessionStoreOperation::ListSubkeys) {
        return Err(SessionStoreHelperError::UnsupportedStore("list_subkeys"));
    }
    let key = main_key(project_key(directory)?, session_id)?;
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for subpath in store.list_subkeys(&key).await? {
        if !subpath.starts_with("subagents/") {
            continue;
        }
        let last = subpath.rsplit('/').next().unwrap_or_default();
        if let Some(agent_id) = last.strip_prefix("agent-")
            && seen.insert(agent_id.to_owned())
        {
            ids.push(agent_id.to_owned());
        }
    }
    Ok(ids)
}

/// Reads a subagent's conversation from a store. Parent ids come from the subagent's last `agent_metadata` entry.
pub async fn get_subagent_messages_from_store(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    agent_id: &str,
    directory: Option<&Path>,
    page: MessagePage,
) -> Result<Vec<SessionMessage>, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() || agent_id.is_empty() {
        return Ok(Vec::new());
    }
    let project = project_key(directory)?;
    let base = main_key(project, session_id)?;
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
    let Ok(subpath) = SessionSubpath::new(subpath) else {
        return Ok(Vec::new());
    };
    let key = base.with_subpath(subpath);
    let entries = load_entries(store, &key).await?;

    let mut metadata: Option<JsonObject> = None;
    let mut transcript = Vec::new();
    for entry in entries {
        if entry.kind() == "agent_metadata" {
            metadata = Some(entry.into_object());
        } else {
            transcript.push(entry);
        }
    }
    if transcript.is_empty() {
        return Ok(Vec::new());
    }
    let (tool_use_id, parent_agent_id) = parent_ids_from_agent_metadata(metadata.as_ref());
    Ok(entries_to_subagent_messages(
        &filter_transcript(transcript),
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
    object.insert("timestamp".into(), Value::from(iso_now()));
    SessionEntry::new(object)
}

/// Renames a session by appending a `custom-title` entry to the store.
pub async fn rename_session_via_store(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    title: &str,
    directory: Option<&Path>,
) -> Result<(), SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Err(invalid_session(session_id));
    }
    let stripped = py_strip(title);
    if stripped.is_empty() {
        return Err(SessionMutationError::EmptyTitle.into());
    }
    let key = main_key(project_key(directory)?, session_id)?;
    let entry = metadata_entry([
        ("type", Value::from("custom-title")),
        ("customTitle", Value::from(stripped)),
        ("sessionId", Value::from(session_id)),
    ]);
    store.append(&key, &[entry]).await?;
    Ok(())
}

/// Tags a session by appending a `tag` entry to the store; `None` clears the tag.
pub async fn tag_session_via_store(
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
            let sanitized = py_strip(&sanitize_unicode(tag)).to_owned();
            if sanitized.is_empty() {
                return Err(SessionMutationError::EmptyTag.into());
            }
            sanitized
        }
        None => String::new(),
    };
    let key = main_key(project_key(directory)?, session_id)?;
    let entry = metadata_entry([
        ("type", Value::from("tag")),
        ("tag", Value::from(tag)),
        ("sessionId", Value::from(session_id)),
    ]);
    store.append(&key, &[entry]).await?;
    Ok(())
}

/// Deletes a session from the store; a no-op for stores without `delete`, which suits append-only backends.
pub async fn delete_session_via_store(
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
    let key = main_key(project_key(directory)?, session_id)?;
    store.delete(&key).await?;
    Ok(())
}

/// Upstream `_derive_title_from_entries`: last custom title, else last AI title, else the first prompt.
fn derive_title_from_entries(entries: &[SessionEntry]) -> Option<String> {
    let (mut custom, mut ai): (Option<&str>, Option<&str>) = (None, None);
    for entry in entries {
        let object = entry.as_object();
        if let Some(title) = object
            .get("customTitle")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            custom = Some(title);
        }
        if let Some(title) = object.get("aiTitle").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            ai = Some(title);
        }
    }
    custom
        .or(ai)
        .map(str::to_owned)
        .or_else(|| Some(extract_first_prompt_from_head(&entries_to_jsonl(entries))))
        .filter(|title| !title.is_empty())
}

/// Forks a stored session into a new session under the same project key, remapping every uuid.
pub async fn fork_session_via_store(
    store: &(impl SessionStore + ?Sized),
    session_id: &str,
    directory: Option<&Path>,
    up_to_message_id: Option<&str>,
    title: Option<&str>,
) -> Result<crate::ForkSessionResult, SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Err(invalid_session(session_id));
    }
    let up_to = up_to_message_id.filter(|id| !id.is_empty());
    if let Some(message_id) = up_to
        && valid_uuid(message_id).is_none()
    {
        return Err(SessionMutationError::InvalidMessageId(message_id.to_owned()).into());
    }
    let project = project_key(directory)?;
    let source = main_key(project.clone(), session_id)?;
    let loaded = load_entries(store, &source).await?;
    if loaded.is_empty() {
        return Err(not_found(session_id));
    }

    let mut transcript = Vec::new();
    let mut replacements = Vec::new();
    for entry in &loaded {
        partition_fork_entry(
            entry.as_object().clone(),
            session_id,
            &mut transcript,
            &mut replacements,
        );
    }
    let (forked, lines) = build_fork_lines(
        transcript,
        replacements,
        session_id,
        up_to,
        title,
        || derive_title_from_entries(&loaded),
        crate::JsonEscaping::Utf8,
    )?;

    let entries = lines
        .iter()
        .map(|line| match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(object)) => Ok(SessionEntry::new(object)),
            Ok(_) => Err(SessionStoreHelperError::NotObject),
            Err(error) => Err(SessionStoreHelperError::Encoding(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let destination = main_key(project, &forked)?;
    store.append(&destination, &entries).await?;
    Ok(crate::ForkSessionResult {
        session_id: SessionId::from(forked),
    })
}

/// Replays a local transcript, and optionally its subagent transcripts and metadata sidecars, into a store. The
/// destination project key is the name of the on-disk project directory the session was found in, so an imported
/// session is indistinguishable from a mirrored one.
pub async fn import_session_to_store(
    files: &SessionFiles,
    session_id: &str,
    store: &(impl SessionStore + ?Sized),
    options: &ImportOptions,
) -> Result<(), SessionStoreHelperError> {
    if valid_uuid(session_id).is_none() {
        return Err(invalid_session(session_id));
    }
    let resolved = files
        .resolve_session_file_path(session_id, options.directory.as_deref())
        .ok_or_else(|| not_found(session_id))?;
    let project_name = resolved
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let project = SessionProjectKey::new(project_name)?;
    let batch_size = if options.batch_size == 0 {
        DEFAULT_IMPORT_BATCH
    } else {
        options.batch_size
    };

    let main = main_key(project.clone(), session_id)?;
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
        let subpath = SessionSubpath::new(parts.join("/")).map_err(SessionKeyError::from)?;
        let key = main.clone().with_subpath(subpath);
        append_jsonl_in_batches(&file, &key, store, batch_size).await?;
        if let Some(mut metadata) = read_agent_metadata_sidecar(&file) {
            metadata.insert("type".into(), Value::from("agent_metadata"));
            store.append(&key, &[SessionEntry::new(metadata)]).await?;
        }
    }
    Ok(())
}

async fn append_jsonl_in_batches(
    path: &Path,
    key: &SessionKey,
    store: &(impl SessionStore + ?Sized),
    batch_size: usize,
) -> Result<(), SessionStoreHelperError> {
    let file = tokio::fs::File::open(path).await.map_err(SessionStoreHelperError::Io)?;
    let mut lines = BufReader::new(file).lines();
    let mut batch: Vec<SessionEntry> = Vec::new();
    let mut bytes = 0_usize;
    while let Some(line) = lines.next_line().await.map_err(SessionStoreHelperError::Io)? {
        if line.is_empty() {
            continue;
        }
        let entry = match serde_json::from_str::<Value>(&line).map_err(SessionStoreHelperError::Encoding)? {
            Value::Object(object) => SessionEntry::new(object),
            _ => return Err(SessionStoreHelperError::NotObject),
        };
        batch.push(entry);
        bytes += line.chars().count();
        if batch.len() >= batch_size || bytes >= MAX_IMPORT_BYTES {
            store.append(key, &batch).await?;
            batch.clear();
            bytes = 0;
        }
    }
    if !batch.is_empty() {
        store.append(key, &batch).await?;
    }
    Ok(())
}

/// Every `*.jsonl` below `base`, sorted by name within each directory.
fn collect_jsonl_files(base: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut files = Vec::new();
    for (name, path) in entries {
        if path.is_dir() {
            files.extend(collect_jsonl_files(&path));
        } else if path.is_file() && name.ends_with(".jsonl") {
            files.push(path);
        }
    }
    files
}
