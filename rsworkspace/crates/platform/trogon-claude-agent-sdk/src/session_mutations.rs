//! Filesystem session mutations: rename, tag, delete and fork.
//!
//! Ports upstream `rename_session`, `tag_session`, `delete_session` and `fork_session`. Rename and tag append a typed
//! metadata line to the transcript, delete removes the transcript and its sibling subagent directory, and fork copies
//! the transcript under fresh uuids.

use crate::constants::{LITE_READ_BUF_SIZE, TRANSCRIPT_TYPES};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

use crate::{
    JsonObject, SessionFiles, SessionId,
    session_files::{
        extract_first_prompt_from_head, extract_first_prompt_from_head_mode, extract_last_json_string_field, valid_uuid,
    },
    session_files_ts::{last_typed_string, sidecar_title},
    session_fork_ts::ForkTsError,
    session_messages_ts::MessagesError,
    session_prompt::PromptMode,
    session_transcript::truthy,
    summary_dialect::SummaryDialect,
};

/// Why a session mutation was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum SessionMutationError {
    #[error("invalid session id: {0}")]
    InvalidSessionId(String),
    #[error("invalid message id: {0}")]
    InvalidMessageId(String),
    #[error("title must be non-empty")]
    EmptyTitle,
    #[error("tag must be non-empty (use None to clear)")]
    EmptyTag,
    #[error("session {session_id} not found")]
    NotFound {
        session_id: String,
        /// The project directory the caller named, when any.
        directory: Option<String>,
    },
    #[error("session {0} has no messages to fork")]
    NoMessages(String),
    #[error("message {message_id} not found in session {session_id}")]
    MessageNotFound { message_id: String, session_id: String },
    #[error("session file I/O failed")]
    Io(#[source] std::io::Error),
    /// The TypeScript chain rebuild threw on malformed compaction metadata.
    #[error("session transcript could not be linked")]
    Messages(#[source] MessagesError),
    /// TypeScript `Ute` kept changing the text for ten rounds; the payload is the first 100 UTF-16 units of the input.
    #[error("Unicode sanitization reached maximum iterations (10) for input: {0}")]
    UnicodeSanitization(String),
}

impl From<ForkTsError> for SessionMutationError {
    fn from(error: ForkTsError) -> Self {
        match error {
            ForkTsError::NoMessages(session_id) => Self::NoMessages(session_id),
            ForkTsError::MessageNotFound { message_id, session_id } => Self::MessageNotFound { message_id, session_id },
            ForkTsError::Messages(error) => Self::Messages(error),
        }
    }
}

/// Which upstream `forkSession` algorithm a fork follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ForkAlgorithm {
    /// The TypeScript SDK: state loader, chain-aware cutoff, data-flow records and a custom title.
    #[default]
    TypeScript,
    /// The Python SDK: line-by-line uuid remapping.
    Python,
}

/// TypeScript `zMe`: the title of a fork when the caller names none.
fn ts_derived_title(content: &[u8], transcript: &Path, session_id: &str) -> String {
    let window = LITE_READ_BUF_SIZE as usize;
    let head = String::from_utf8_lossy(&content[..content.len().min(window)]).into_owned();
    let tail = String::from_utf8_lossy(&content[content.len().saturating_sub(window)..]).into_owned();
    let typed =
        |text: &str, ty: &str, key: &str| last_typed_string(text, key, Some(ty), true).filter(|v| !v.is_empty());
    let own = last_typed_string(&tail, "customTitle", Some("custom-title"), true);
    let custom = if own.is_some() {
        own
    } else {
        sidecar_title(transcript, session_id).or_else(|| typed(&head, "custom-title", "customTitle"))
    };
    custom
        .filter(|title| !title.is_empty())
        .or_else(|| typed(&tail, "ai-title", "aiTitle"))
        .or_else(|| typed(&head, "ai-title", "aiTitle"))
        .unwrap_or_else(|| extract_first_prompt_from_head_mode(&head, SummaryDialect::TypeScript, PromptMode::Shown))
}

/// The new session created by a fork (`ForkSessionResult` upstream).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkSessionResult {
    pub session_id: SessionId,
}

/// How non-ASCII characters are written when a fork is serialized to disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JsonEscaping {
    /// Raw UTF-8, as TypeScript `JSON.stringify`.
    #[default]
    Utf8,
    /// `\uXXXX` escapes with surrogate pairs, as Python `json.dumps` with its default `ensure_ascii`.
    Ascii,
}

impl JsonEscaping {
    pub(crate) fn render(self, value: &Value) -> String {
        let text = value.to_string();
        match self {
            Self::Utf8 => text,
            Self::Ascii => {
                let mut out = String::with_capacity(text.len());
                for ch in text.chars() {
                    if ch.is_ascii() {
                        out.push(ch);
                    } else {
                        let mut units = [0_u16; 2];
                        for unit in ch.encode_utf16(&mut units) {
                            out.push_str(&format!("\\u{unit:04x}"));
                        }
                    }
                }
                out
            }
        }
    }
}

impl SessionFiles {
    /// Renames a session by appending a `custom-title` line; the last one wins when listing.
    pub fn rename_session(
        &self,
        session_id: &str,
        title: &str,
        directory: Option<&Path>,
    ) -> Result<(), SessionMutationError> {
        let session_id = checked_session_id(session_id)?;
        let stripped = py_strip(title);
        if stripped.is_empty() {
            return Err(SessionMutationError::EmptyTitle);
        }
        let line = compact_line(
            &[
                ("type", Value::from("custom-title")),
                ("customTitle", Value::from(stripped)),
                ("sessionId", Value::from(session_id)),
            ],
            JsonEscaping::Utf8,
        );
        self.append_to_session(session_id, &line, directory)
    }

    /// Tags a session; `None` clears the tag with an empty tag line. Tags are Unicode-sanitized first.
    pub fn tag_session(
        &self,
        session_id: &str,
        tag: Option<&str>,
        directory: Option<&Path>,
    ) -> Result<(), SessionMutationError> {
        let session_id = checked_session_id(session_id)?;
        let tag = match tag {
            Some(tag) => {
                let sanitized = py_strip(&sanitize_unicode(tag)).to_owned();
                if sanitized.is_empty() {
                    return Err(SessionMutationError::EmptyTag);
                }
                sanitized
            }
            None => String::new(),
        };
        let line = compact_line(
            &[
                ("type", Value::from("tag")),
                ("tag", Value::from(tag)),
                ("sessionId", Value::from(session_id)),
            ],
            JsonEscaping::Utf8,
        );
        self.append_to_session(session_id, &line, directory)
    }

    /// Hard-deletes a session: the transcript and its sibling `<session id>/` subagent directory.
    pub fn delete_session(&self, session_id: &str, directory: Option<&Path>) -> Result<(), SessionMutationError> {
        let session_id = checked_session_id(session_id)?;
        let (path, _) = self
            .find_session_file(session_id, directory)
            .ok_or_else(|| not_found(session_id, directory))?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(SessionMutationError::NotFound {
                    session_id: session_id.to_owned(),
                    directory: None,
                });
            }
            Err(error) => return Err(SessionMutationError::Io(error)),
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent.join(session_id));
        }
        Ok(())
    }

    /// Forks a session into a new transcript beside the source, remapping every uuid, with the TypeScript algorithm
    /// (`forkSession` of the TypeScript SDK).
    pub fn fork_session(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        up_to_message_id: Option<&str>,
        title: Option<&str>,
    ) -> Result<ForkSessionResult, SessionMutationError> {
        self.fork_session_using(
            ForkAlgorithm::TypeScript,
            session_id,
            directory,
            up_to_message_id,
            title,
        )
    }

    /// [`SessionFiles::fork_session`] with an explicit choice between the TypeScript and Python algorithms.
    pub fn fork_session_using(
        &self,
        algorithm: ForkAlgorithm,
        session_id: &str,
        directory: Option<&Path>,
        up_to_message_id: Option<&str>,
        title: Option<&str>,
    ) -> Result<ForkSessionResult, SessionMutationError> {
        match algorithm {
            ForkAlgorithm::TypeScript => self.fork_session_ts(session_id, directory, up_to_message_id, title),
            ForkAlgorithm::Python => {
                self.fork_session_with(session_id, directory, up_to_message_id, title, JsonEscaping::Ascii)
            }
        }
    }

    /// TypeScript `forkSession` (`IB`): no message id validation, the `ei` lookup order, the state loader and the
    /// `NB` rewrite, written with mode 0600 and the `w` flag.
    fn fork_session_ts(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        up_to_message_id: Option<&str>,
        title: Option<&str>,
    ) -> Result<ForkSessionResult, SessionMutationError> {
        let session_id = checked_session_id(session_id)?;
        let (file_path, _) = self
            .resolve_session_ts(session_id, directory)
            .ok_or_else(|| not_found(session_id, directory))?;
        let content = std::fs::read(&file_path).map_err(SessionMutationError::Io)?;
        if content.is_empty() {
            return Err(not_found(session_id, directory));
        }
        let state = crate::session_fork_ts::load_state(&content, session_id);
        let derive_title = || ts_derived_title(&content, &file_path, session_id);
        let mut ids = || uuid::Uuid::new_v4().to_string();
        let mut clock = || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
                .unwrap_or(0)
        };
        let forked = crate::session_fork_ts::fork_entries(
            &state,
            &crate::session_fork_ts::ForkTsRequest {
                session_id,
                up_to_message_id,
                title,
            },
            derive_title,
            &mut ids,
            &mut clock,
        )?;
        let project_dir = file_path.parent().map(Path::to_path_buf).unwrap_or_default();
        let fork_path = project_dir.join(format!("{}.jsonl", forked.session_id));
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        }
        let mut file = options.open(&fork_path).map_err(SessionMutationError::Io)?;
        file.write_all(crate::session_fork_ts::serialize_entries(&forked.entries).as_bytes())
            .map_err(SessionMutationError::Io)?;
        Ok(ForkSessionResult {
            session_id: SessionId::from(forked.session_id),
        })
    }

    /// The Python algorithm with a selectable serialization of non-ASCII text.
    pub fn fork_session_with(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        up_to_message_id: Option<&str>,
        title: Option<&str>,
        escaping: JsonEscaping,
    ) -> Result<ForkSessionResult, SessionMutationError> {
        let session_id = checked_session_id(session_id)?;
        if let Some(message_id) = up_to_message_id.filter(|id| !id.is_empty())
            && valid_uuid(message_id).is_none()
        {
            return Err(SessionMutationError::InvalidMessageId(message_id.to_owned()));
        }
        let (file_path, project_dir) = self
            .find_session_file(session_id, directory)
            .ok_or_else(|| not_found(session_id, directory))?;
        let content = std::fs::read(&file_path).map_err(SessionMutationError::Io)?;
        if content.is_empty() {
            return Err(SessionMutationError::NoMessages(session_id.to_owned()));
        }
        let text = String::from_utf8_lossy(&content);
        let (transcript, replacements) = parse_fork_entries(text.lines_py(), session_id);

        let derive_title = || {
            let buffer = LITE_READ_BUF_SIZE as usize;
            let head = String::from_utf8_lossy(&content[..content.len().min(buffer)]).into_owned();
            let tail = String::from_utf8_lossy(&content[content.len().saturating_sub(buffer)..]).into_owned();
            extract_last_json_string_field(&tail, "customTitle")
                .or_else(|| extract_last_json_string_field(&head, "customTitle"))
                .or_else(|| extract_last_json_string_field(&tail, "aiTitle"))
                .or_else(|| extract_last_json_string_field(&head, "aiTitle"))
                .or_else(|| Some(extract_first_prompt_from_head(&head)))
                .filter(|title| !title.is_empty())
        };
        let (forked, lines) = build_fork_lines(
            transcript,
            replacements,
            session_id,
            up_to_message_id.filter(|id| !id.is_empty()),
            title,
            derive_title,
            escaping,
        )?;

        let fork_path = project_dir.join(format!("{forked}.jsonl"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        }
        let mut file = options.open(&fork_path).map_err(SessionMutationError::Io)?;
        file.write_all(format!("{}\n", lines.join("\n")).as_bytes())
            .map_err(SessionMutationError::Io)?;
        Ok(ForkSessionResult {
            session_id: SessionId::from(forked),
        })
    }

    /// The transcript file and its project directory; empty files do not count, as upstream.
    fn find_session_file(&self, session_id: &str, directory: Option<&Path>) -> Option<(PathBuf, PathBuf)> {
        let file_name = format!("{session_id}.jsonl");
        self.session_candidate_dirs(directory).into_iter().find_map(|dir| {
            let path = dir.join(&file_name);
            std::fs::metadata(&path)
                .ok()
                .filter(|metadata| metadata.len() > 0)
                .map(|_| (path, dir))
        })
    }

    fn append_to_session(
        &self,
        session_id: &str,
        data: &str,
        directory: Option<&Path>,
    ) -> Result<(), SessionMutationError> {
        let file_name = format!("{session_id}.jsonl");
        for dir in self.session_candidate_dirs(directory) {
            if try_append(&dir.join(&file_name), data).map_err(SessionMutationError::Io)? {
                return Ok(());
            }
        }
        Err(not_found(session_id, directory))
    }

    fn session_candidate_dirs(&self, directory: Option<&Path>) -> Vec<PathBuf> {
        if let Some(directory) = directory.filter(|directory| !directory.as_os_str().is_empty()) {
            let canonical = crate::session_project_key::canonicalize_lenient(directory);
            return self
                .directory_candidates(&canonical)
                .into_iter()
                .map(|(dir, _)| dir)
                .collect();
        }
        self.project_dirs()
    }
}

fn checked_session_id(session_id: &str) -> Result<&str, SessionMutationError> {
    valid_uuid(session_id).ok_or_else(|| SessionMutationError::InvalidSessionId(session_id.to_owned()))
}

fn not_found(session_id: &str, directory: Option<&Path>) -> SessionMutationError {
    SessionMutationError::NotFound {
        session_id: session_id.to_owned(),
        directory: directory
            .filter(|directory| !directory.as_os_str().is_empty())
            .map(|directory| directory.to_string_lossy().into_owned()),
    }
}

/// Appends to an existing non-empty file without creating it. `false` means "not here, keep searching".
fn try_append(path: &Path, data: &str) -> std::io::Result<bool> {
    let mut file = match OpenOptions::new().append(true).open(path) {
        Ok(file) => file,
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => return Ok(false),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    file.write_all(data.as_bytes())?;
    Ok(true)
}

/// A compact JSON object whose keys keep the given order, so scanners that match a literal line prefix keep working.
fn compact_line(fields: &[(&str, Value)], escaping: JsonEscaping) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{}:{}", escaping.render(&Value::from(*key)), escaping.render(value)))
        .collect();
    format!("{{{}}}\n", body.join(","))
}

/// Python `str.strip()`: Unicode whitespace plus the information separators `\x1c` to `\x1f`.
pub(crate) fn py_strip(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Upstream `_sanitize_unicode`: iterates NFKC and removal of format, private-use and unassigned characters until
/// stable, at most ten rounds.
pub(crate) fn sanitize_unicode(value: &str) -> String {
    let mut current = value.to_owned();
    for _ in 0..10 {
        let previous = current.clone();
        current = previous
            .nfkc()
            .filter(|c| {
                !matches!(
                    get_general_category(*c),
                    GeneralCategory::Format | GeneralCategory::PrivateUse | GeneralCategory::Unassigned
                )
            })
            .filter(|c| {
                !matches!(*c,
                    '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
                    | '\u{e000}'..='\u{f8ff}')
            })
            .collect();
        if current == previous {
            break;
        }
    }
    current
}

/// TypeScript `Ute`: ASCII passes through; anything else is NFKC-normalized and stripped of format, private-use and
/// unassigned characters (`Lte`) for ten rounds at most. Reaching the tenth round is an error even when the text has
/// settled by then, as upstream.
pub(crate) fn sanitize_unicode_ts(value: &str) -> Result<String, SessionMutationError> {
    if value.is_ascii() {
        return Ok(value.to_owned());
    }
    let mut current = value.to_owned();
    let mut previous = String::new();
    let mut rounds = 0;
    while current != previous && rounds < 10 {
        previous = current;
        current = strip_unassigned(&previous.nfkc().collect::<String>());
        rounds += 1;
    }
    if rounds >= 10 {
        let units: Vec<u16> = value.encode_utf16().take(100).collect();
        return Err(SessionMutationError::UnicodeSanitization(String::from_utf16_lossy(
            &units,
        )));
    }
    Ok(current)
}

/// TypeScript `Lte`: drops `\p{Cf}`, `\p{Co}` and `\p{Cn}` plus the bidi, zero-width and private-use ranges.
fn strip_unassigned(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(
                get_general_category(*c),
                GeneralCategory::Format | GeneralCategory::PrivateUse | GeneralCategory::Unassigned
            ) && !matches!(*c,
                '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
                | '\u{e000}'..='\u{f8ff}')
        })
        .collect()
}

/// Python `str.splitlines()`, which also breaks on `\r`, vertical tab, form feed, the information separators,
/// `\x85`, and the Unicode line and paragraph separators.
trait LinesPy {
    fn lines_py(&self) -> Vec<&str>;
}

impl LinesPy for str {
    fn lines_py(&self) -> Vec<&str> {
        let mut lines = Vec::new();
        let mut start = 0;
        let mut chars = self.char_indices().peekable();
        while let Some((index, c)) = chars.next() {
            if matches!(
                c,
                '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
            ) {
                lines.push(&self[start..index]);
                start = index + c.len_utf8();
                if c == '\r'
                    && let Some(&(next, '\n')) = chars.peek()
                {
                    chars.next();
                    start = next + 1;
                }
            }
        }
        if start < self.len() {
            lines.push(&self[start..]);
        }
        lines
    }
}

/// Keeps transcript entries (those with a string uuid) and collects this session's content replacements.
pub(crate) fn parse_fork_entries(lines: Vec<&str>, session_id: &str) -> (Vec<JsonObject>, Vec<Value>) {
    let mut transcript = Vec::new();
    let mut replacements = Vec::new();
    for line in lines {
        let line = py_strip(line);
        if line.is_empty() {
            continue;
        }
        let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        partition_fork_entry(entry, session_id, &mut transcript, &mut replacements);
    }
    (transcript, replacements)
}

/// Shared by the filesystem and store-backed forks.
pub(crate) fn partition_fork_entry(
    entry: JsonObject,
    session_id: &str,
    transcript: &mut Vec<JsonObject>,
    replacements: &mut Vec<Value>,
) {
    let entry_type = entry.get("type").and_then(Value::as_str);
    if entry_type.is_some_and(|kind| TRANSCRIPT_TYPES.contains(&kind))
        && entry.get("uuid").is_some_and(Value::is_string)
    {
        transcript.push(entry);
    } else if entry_type == Some("content-replacement")
        && entry.get("sessionId").and_then(Value::as_str) == Some(session_id)
        && let Some(Value::Array(items)) = entry.get("replacements")
    {
        replacements.extend(items.iter().cloned());
    }
}

pub(crate) fn iso_now() -> String {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let (days, seconds) = (elapsed.as_secs() / 86_400, elapsed.as_secs() % 86_400);
    let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(0));
    let micros = elapsed.subsec_micros();
    let fraction = if micros == 0 {
        String::new()
    } else {
        format!(".{micros:06}")
    };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{fraction}Z",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Upstream `_build_fork_lines`: remaps uuids and serializes the fork. Returns the new session id and compact JSON
/// lines without trailing newlines. `derive_title` runs only when no explicit title is given.
pub(crate) fn build_fork_lines(
    transcript: Vec<JsonObject>,
    content_replacements: Vec<Value>,
    session_id: &str,
    up_to_message_id: Option<&str>,
    title: Option<&str>,
    derive_title: impl FnOnce() -> Option<String>,
    escaping: JsonEscaping,
) -> Result<(String, Vec<String>), SessionMutationError> {
    let no_messages = || SessionMutationError::NoMessages(session_id.to_owned());
    let mut transcript: Vec<JsonObject> = transcript
        .into_iter()
        .filter(|entry| !truthy(entry.get("isSidechain")))
        .collect();
    if transcript.is_empty() {
        return Err(no_messages());
    }
    if let Some(message_id) = up_to_message_id {
        let cutoff = transcript
            .iter()
            .position(|entry| entry.get("uuid").and_then(Value::as_str) == Some(message_id))
            .ok_or_else(|| SessionMutationError::MessageNotFound {
                message_id: message_id.to_owned(),
                session_id: session_id.to_owned(),
            })?;
        transcript.truncate(cutoff + 1);
    }

    let uuid_of = |entry: &JsonObject| entry.get("uuid").and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut mapping: HashMap<String, String> = HashMap::new();
    for entry in &transcript {
        mapping.insert(uuid_of(entry), uuid::Uuid::new_v4().to_string());
    }
    let writable: Vec<&JsonObject> = transcript
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) != Some("progress"))
        .collect();
    if writable.is_empty() {
        return Err(no_messages());
    }
    let mut by_uuid: HashMap<String, &JsonObject> = HashMap::new();
    for entry in &transcript {
        by_uuid.insert(uuid_of(entry), entry);
    }

    let forked_session_id = uuid::Uuid::new_v4().to_string();
    let now = iso_now();
    let mut lines = Vec::new();
    for (index, original) in writable.iter().enumerate() {
        let new_uuid = mapping.get(&uuid_of(original)).cloned().unwrap_or_default();

        let mut new_parent = Value::Null;
        let mut parent_id = original
            .get("parentUuid")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        while let Some(id) = parent_id {
            let Some(parent) = by_uuid.get(id) else {
                break;
            };
            if parent.get("type").and_then(Value::as_str) != Some("progress") {
                new_parent = mapping.get(id).cloned().map_or(Value::Null, Value::from);
                break;
            }
            parent_id = parent
                .get("parentUuid")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty());
        }

        let timestamp = if index == writable.len() - 1 {
            Value::from(now.clone())
        } else {
            original
                .get("timestamp")
                .cloned()
                .unwrap_or_else(|| Value::from(now.clone()))
        };

        let logical_parent = original.get("logicalParentUuid").cloned().unwrap_or(Value::Null);
        let new_logical_parent = if truthy(Some(&logical_parent)) {
            logical_parent
                .as_str()
                .and_then(|id| mapping.get(id))
                .cloned()
                .map_or(Value::Null, Value::from)
        } else {
            logical_parent
        };

        let mut forked: JsonObject = (*original).clone();
        forked.insert("uuid".into(), Value::from(new_uuid));
        forked.insert("parentUuid".into(), new_parent);
        forked.insert("logicalParentUuid".into(), new_logical_parent);
        forked.insert("sessionId".into(), Value::from(forked_session_id.clone()));
        forked.insert("timestamp".into(), timestamp);
        forked.insert("isSidechain".into(), Value::Bool(false));
        let mut source = JsonObject::new();
        source.insert("sessionId".into(), Value::from(session_id));
        source.insert("messageUuid".into(), Value::from(uuid_of(original)));
        forked.insert("forkedFrom".into(), Value::Object(source));
        for key in ["teamName", "agentName", "slug", "sourceToolAssistantUUID"] {
            forked.shift_remove(key);
        }
        lines.push(escaping.render(&Value::Object(forked)));
    }

    if !content_replacements.is_empty() {
        lines.push(
            compact_line(
                &[
                    ("type", Value::from("content-replacement")),
                    ("sessionId", Value::from(forked_session_id.clone())),
                    ("replacements", Value::Array(content_replacements)),
                    ("uuid", Value::from(uuid::Uuid::new_v4().to_string())),
                    ("timestamp", Value::from(now.clone())),
                ],
                escaping,
            )
            .trim_end()
            .to_owned(),
        );
    }

    let fork_title = title
        .map(py_strip)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{} (fork)", derive_title().unwrap_or_else(|| "Forked session".into())));
    lines.push(
        compact_line(
            &[
                ("type", Value::from("custom-title")),
                ("sessionId", Value::from(forked_session_id.clone())),
                ("customTitle", Value::from(fork_title)),
                ("uuid", Value::from(uuid::Uuid::new_v4().to_string())),
                ("timestamp", Value::from(now)),
            ],
            escaping,
        )
        .trim_end()
        .to_owned(),
    );
    Ok((forked_session_id, lines))
}

fn environment_files() -> Option<SessionFiles> {
    SessionFiles::from_environment()
}

fn unresolved(session_id: &str) -> SessionMutationError {
    SessionMutationError::NotFound {
        session_id: session_id.to_owned(),
        directory: None,
    }
}

/// [`SessionFiles::rename_session`] over the environment's config directory.
pub fn rename_session(session_id: &str, title: &str, directory: Option<&Path>) -> Result<(), SessionMutationError> {
    environment_files()
        .ok_or_else(|| unresolved(session_id))?
        .rename_session(session_id, title, directory)
}

/// [`SessionFiles::tag_session`] over the environment's config directory.
pub fn tag_session(session_id: &str, tag: Option<&str>, directory: Option<&Path>) -> Result<(), SessionMutationError> {
    environment_files()
        .ok_or_else(|| unresolved(session_id))?
        .tag_session(session_id, tag, directory)
}

/// [`SessionFiles::delete_session`] over the environment's config directory.
pub fn delete_session(session_id: &str, directory: Option<&Path>) -> Result<(), SessionMutationError> {
    environment_files()
        .ok_or_else(|| unresolved(session_id))?
        .delete_session(session_id, directory)
}

/// [`SessionFiles::fork_session`] over the environment's config directory.
pub fn fork_session(
    session_id: &str,
    directory: Option<&Path>,
    up_to_message_id: Option<&str>,
    title: Option<&str>,
) -> Result<ForkSessionResult, SessionMutationError> {
    environment_files().ok_or_else(|| unresolved(session_id))?.fork_session(
        session_id,
        directory,
        up_to_message_id,
        title,
    )
}
