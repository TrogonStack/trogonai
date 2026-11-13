//! Filesystem session readers over `<config dir>/projects/<sanitized cwd>/<session id>.jsonl`.
//!
//! Ports upstream `list_sessions`, `get_session_info`, `get_session_messages`, `list_subagents` and
//! `get_subagent_messages`. Metadata comes from `stat` plus a 64 KiB head and tail read of each file, never from a
//! full parse, exactly as upstream does.

use crate::adapters::{EnvironmentVariable, SystemEnvironment};
use crate::constants::{LITE_READ_BUF_SIZE, MAX_SANITIZED_LENGTH};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::{
    JsonObject, SdkSessionInfo, SessionId, SummaryDialect,
    session_project_key::{canonicalize_lenient, sanitize_path},
    session_prompt::{entry_first_prompt, iso_to_epoch_ms},
    session_transcript::MessagePage,
};

/// Arguments of [`SessionFiles::list_sessions`] (`ListSessionsOptions` upstream).
#[derive(Clone, Debug)]
pub struct ListSessionsOptions {
    /// Project directory to list; every project is listed when `None`.
    pub directory: Option<PathBuf>,
    /// Maximum number of sessions, where `None` or `Some(0)` means all.
    pub limit: Option<usize>,
    pub offset: usize,
    /// Include sessions of the sibling git worktrees of `directory`.
    pub include_worktrees: bool,
    /// Keep sessions started by the SDKs or a daemon. Only TypeScript rules filter them, so the setting is
    /// ignored under [`SummaryDialect::Python`].
    pub include_programmatic: bool,
    /// Which upstream SDK's rules read the config directory.
    pub dialect: SummaryDialect,
}

impl Default for ListSessionsOptions {
    fn default() -> Self {
        Self {
            directory: None,
            limit: None,
            offset: 0,
            include_worktrees: true,
            include_programmatic: true,
            dialect: SummaryDialect::TypeScript,
        }
    }
}

/// Reader over one Claude config directory's `projects` tree.
#[derive(Clone, Debug)]
pub struct SessionFiles {
    projects_dir: PathBuf,
    pub(crate) precompact_skip: bool,
}

impl SessionFiles {
    /// Resolves the directory like the CLI: `CLAUDE_CONFIG_DIR`, else `~/.claude`, NFC normalized. `None` when
    /// neither the variable nor a home directory is set.
    pub fn from_environment() -> Option<Self> {
        Self::from_environment_with(&SystemEnvironment)
    }

    /// [`SessionFiles::from_environment`] over an explicit environment source.
    pub fn from_environment_with<E: EnvironmentVariable>(environment: &E) -> Option<Self> {
        let configured = environment
            .var_os(OsStr::new("CLAUDE_CONFIG_DIR"))
            .filter(|value| !value.is_empty());
        let config = match configured {
            Some(value) => PathBuf::from(value),
            None => {
                let home = environment.var_os(OsStr::new(if cfg!(windows) { "USERPROFILE" } else { "HOME" }))?;
                PathBuf::from(home).join(".claude")
            }
        };
        let mut files = Self::with_config_dir(config);
        files.precompact_skip = !crate::session_precompact::skip_disabled(environment);
        Some(files)
    }

    /// Whether transcripts above 5 MiB skip everything before the last compact boundary. On by default;
    /// [`SessionFiles::from_environment_with`] turns it off when `CLAUDE_CODE_DISABLE_PRECOMPACT_SKIP` is truthy.
    #[must_use]
    pub fn with_precompact_skip(mut self, enabled: bool) -> Self {
        self.precompact_skip = enabled;
        self
    }

    pub fn with_config_dir(config_dir: impl AsRef<Path>) -> Self {
        let normalized: String = config_dir.as_ref().to_string_lossy().nfc().collect();
        Self {
            projects_dir: PathBuf::from(normalized).join("projects"),
            precompact_skip: true,
        }
    }

    pub fn projects_dir(&self) -> &Path {
        &self.projects_dir
    }

    /// Lists sessions under the Python summary rules, newest first, with ties broken by session id.
    pub(crate) fn list_sessions_python(&self, options: &ListSessionsOptions) -> Vec<SdkSessionInfo> {
        let sessions = match options
            .directory
            .as_deref()
            .filter(|directory| !directory.as_os_str().is_empty())
        {
            Some(directory) => self.sessions_for_project(directory, options.include_worktrees),
            None => self.all_sessions(),
        };
        sort_page(sessions, options.limit, options.offset)
    }

    /// Reads one session's metadata under the Python summary rules.
    pub(crate) fn get_session_info_python(&self, session_id: &str, directory: Option<&Path>) -> Option<SdkSessionInfo> {
        let session_id = valid_uuid(session_id)?;
        let file_name = format!("{session_id}.jsonl");
        if let Some(directory) = directory.filter(|directory| !directory.as_os_str().is_empty()) {
            let canonical = canonicalize_lenient(directory);
            for (candidate, project_path) in self.directory_candidates(&canonical) {
                if let Some(lite) = read_session_lite(&candidate.join(&file_name)) {
                    return parse_session_info_from_lite(session_id, &lite, Some(&project_path));
                }
            }
            return None;
        }
        self.project_dirs()
            .into_iter()
            .find_map(|entry| read_session_lite(&entry.join(&file_name)))
            .and_then(|lite| parse_session_info_from_lite(session_id, &lite, None))
    }

    pub(crate) fn project_dirs(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.projects_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Finds the project directory for a path, tolerating the hash mismatch upstream documents for paths longer
    /// than 200 characters (the CLI and the Node SDK hash differently, so fall back to the shared prefix).
    pub fn find_project_dir(&self, project_path: &str) -> Option<PathBuf> {
        let exact = self
            .projects_dir
            .join(sanitize_path(project_path, SummaryDialect::Python));
        if exact.is_dir() {
            return Some(exact);
        }
        let sanitized = sanitize_path(project_path, SummaryDialect::Python);
        if sanitized.len() <= MAX_SANITIZED_LENGTH {
            return None;
        }
        let prefix = format!("{}-", &sanitized[..MAX_SANITIZED_LENGTH]);
        let mut matches: Vec<PathBuf> = std::fs::read_dir(&self.projects_dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_dir()
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
            })
            .collect();
        matches.sort();
        matches.into_iter().next()
    }

    /// The directory itself first, then the other git worktrees of its repository.
    pub(crate) fn directory_candidates(&self, canonical: &str) -> Vec<(PathBuf, String)> {
        let mut candidates = Vec::new();
        if let Some(dir) = self.find_project_dir(canonical) {
            candidates.push((dir, canonical.to_owned()));
        }
        for worktree in worktree_paths(canonical) {
            if worktree == canonical {
                continue;
            }
            if let Some(dir) = self.find_project_dir(&worktree) {
                candidates.push((dir, worktree));
            }
        }
        candidates
    }

    fn sessions_for_project(&self, directory: &Path, include_worktrees: bool) -> Vec<SdkSessionInfo> {
        let canonical = canonicalize_lenient(directory);
        let worktrees = if include_worktrees {
            worktree_paths(&canonical)
        } else {
            Vec::new()
        };
        if worktrees.len() <= 1 {
            return self
                .find_project_dir(&canonical)
                .map(|dir| read_sessions_from_dir(&dir, Some(&canonical)))
                .unwrap_or_default();
        }

        let case_insensitive = cfg!(windows);
        let fold = |name: &str| {
            if case_insensitive {
                name.to_lowercase()
            } else {
                name.to_owned()
            }
        };
        let mut indexed: Vec<(String, String)> = worktrees
            .into_iter()
            .map(|path| {
                let prefix = fold(&sanitize_path(&path, SummaryDialect::Python));
                (path, prefix)
            })
            .collect();
        indexed.sort_by_key(|(_, prefix)| std::cmp::Reverse(prefix.chars().count()));

        let mut sessions = Vec::new();
        let mut seen_dirs: HashSet<String> = HashSet::new();
        if let Some(own) = self.find_project_dir(&canonical) {
            if let Some(name) = own.file_name() {
                seen_dirs.insert(fold(&name.to_string_lossy()));
            }
            sessions.extend(read_sessions_from_dir(&own, Some(&canonical)));
        }
        let mut entries = self.project_dirs();
        entries.sort();
        for entry in entries {
            let name = fold(
                &entry
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            if seen_dirs.contains(&name) {
                continue;
            }
            for (worktree, prefix) in &indexed {
                let matches = name == *prefix
                    || (prefix.chars().count() >= MAX_SANITIZED_LENGTH && name.starts_with(&format!("{prefix}-")));
                if matches {
                    seen_dirs.insert(name.clone());
                    sessions.extend(read_sessions_from_dir(&entry, Some(worktree)));
                    break;
                }
            }
        }
        deduplicate(sessions)
    }

    fn all_sessions(&self) -> Vec<SdkSessionInfo> {
        let mut sessions = Vec::new();
        for dir in self.project_dirs() {
            sessions.extend(read_sessions_from_dir(&dir, None));
        }
        deduplicate(sessions)
    }

    pub(crate) fn read_session_file(&self, session_id: &str, directory: Option<&Path>) -> Option<String> {
        let file_name = format!("{session_id}.jsonl");
        let read = |dir: &Path| {
            std::fs::read_to_string(dir.join(&file_name))
                .ok()
                .filter(|content| !content.is_empty())
        };
        if let Some(directory) = directory.filter(|directory| !directory.as_os_str().is_empty()) {
            let canonical = canonicalize_lenient(directory);
            return self
                .directory_candidates(&canonical)
                .into_iter()
                .find_map(|(dir, _)| read(&dir));
        }
        let Ok(entries) = std::fs::read_dir(&self.projects_dir) else {
            return None;
        };
        entries.filter_map(Result::ok).find_map(|entry| read(&entry.path()))
    }

    pub(crate) fn resolve_session_file_path(&self, session_id: &str, directory: Option<&Path>) -> Option<PathBuf> {
        let file_name = format!("{session_id}.jsonl");
        let non_empty = |dir: &Path| {
            let candidate = dir.join(&file_name);
            std::fs::metadata(&candidate)
                .ok()
                .filter(|metadata| metadata.len() > 0)
                .map(|_| candidate)
        };
        if let Some(directory) = directory.filter(|directory| !directory.as_os_str().is_empty()) {
            let canonical = canonicalize_lenient(directory);
            return self
                .directory_candidates(&canonical)
                .into_iter()
                .find_map(|(dir, _)| non_empty(&dir));
        }
        self.project_dirs().into_iter().find_map(|dir| non_empty(&dir))
    }
}

pub(crate) fn valid_uuid(candidate: &str) -> Option<&str> {
    let groups: Vec<&str> = candidate.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    (groups.len() == 5
        && groups
            .iter()
            .zip(lengths)
            .all(|(group, length)| group.len() == length && group.bytes().all(|byte| byte.is_ascii_hexdigit())))
    .then_some(candidate)
}

fn deduplicate(sessions: Vec<SdkSessionInfo>) -> Vec<SdkSessionInfo> {
    let mut by_id: HashMap<String, SdkSessionInfo> = HashMap::new();
    for session in sessions {
        let key = session.session_id.as_str().to_owned();
        match by_id.get(&key) {
            Some(existing) if existing.last_modified >= session.last_modified => {}
            _ => {
                by_id.insert(key, session);
            }
        }
    }
    by_id.into_values().collect()
}

pub(crate) fn sort_page(mut sessions: Vec<SdkSessionInfo>, limit: Option<usize>, offset: usize) -> Vec<SdkSessionInfo> {
    sessions.sort_by(|a, b| {
        b.last_modified
            .cmp(&a.last_modified)
            .then_with(|| b.session_id.as_str().cmp(a.session_id.as_str()))
    });
    let page = MessagePage { limit, offset };
    match page.limit {
        Some(limit) if limit > 0 => sessions.into_iter().skip(offset).take(limit).collect(),
        _ => sessions.into_iter().skip(offset).collect(),
    }
}

fn read_sessions_from_dir(dir: &Path, project_path: Option<&str>) -> Vec<SdkSessionInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let session_id = valid_uuid(name.strip_suffix(".jsonl")?)?.to_owned();
            let lite = read_session_lite(&entry.path())?;
            parse_session_info_from_lite(&session_id, &lite, project_path)
        })
        .collect()
}

pub(crate) struct LiteSessionFile {
    pub(crate) mtime: u64,
    pub(crate) size: u64,
    pub(crate) head: String,
    pub(crate) tail: String,
}

/// Stats a session file and reads its first and last 64 KiB, or `None` when it is empty or unreadable.
pub(crate) fn read_session_lite(path: &Path) -> Option<LiteSessionFile> {
    let mut file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let size = metadata.len();
    let modified = metadata.modified().ok()?;
    let mtime = match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => u64::try_from(after.as_millis()).unwrap_or(u64::MAX),
        Err(_) => 0,
    };
    let mut head_bytes = Vec::new();
    (&mut file).take(LITE_READ_BUF_SIZE).read_to_end(&mut head_bytes).ok()?;
    if head_bytes.is_empty() {
        return None;
    }
    let head = String::from_utf8_lossy(&head_bytes).into_owned();
    let tail_offset = size.saturating_sub(LITE_READ_BUF_SIZE);
    let tail = if tail_offset == 0 {
        head.clone()
    } else {
        file.seek(SeekFrom::Start(tail_offset)).ok()?;
        let mut tail_bytes = Vec::new();
        (&mut file).take(LITE_READ_BUF_SIZE).read_to_end(&mut tail_bytes).ok()?;
        String::from_utf8_lossy(&tail_bytes).into_owned()
    };
    Some(LiteSessionFile {
        mtime,
        size,
        head,
        tail,
    })
}

pub(crate) fn unescape_json_string(raw: &str) -> String {
    if !raw.contains('\\') {
        return raw.to_owned();
    }
    serde_json::from_str::<String>(&format!("\"{raw}\"")).unwrap_or_else(|_| raw.to_owned())
}

pub(crate) fn string_end(text: &str, value_start: usize) -> (Option<usize>, usize) {
    let bytes = text.as_bytes();
    let mut i = value_start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return (Some(i), i),
            _ => i += 1,
        }
    }
    (None, i)
}

/// First `"key":"value"` string value, without parsing, so it works on truncated lines.
pub(crate) fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    for pattern in [format!("\"{key}\":\""), format!("\"{key}\": \"")] {
        let Some(index) = text.find(&pattern) else { continue };
        let value_start = index + pattern.len();
        if let (Some(end), _) = string_end(text, value_start) {
            return Some(unescape_json_string(&text[value_start..end]));
        }
    }
    None
}

/// Like [`extract_json_string_field`] but keeps the last occurrence of each key spelling.
pub(crate) fn extract_last_json_string_field(text: &str, key: &str) -> Option<String> {
    let mut last = None;
    for pattern in [format!("\"{key}\":\""), format!("\"{key}\": \"")] {
        let mut search_from = 0;
        while let Some(found) = text.get(search_from..).and_then(|rest| rest.find(&pattern)) {
            let value_start = search_from + found + pattern.len();
            let (end, stopped) = string_end(text, value_start);
            if let Some(end) = end {
                last = Some(unescape_json_string(&text[value_start..end]));
            }
            search_from = stopped + 1;
        }
    }
    last
}

pub(crate) fn extract_first_prompt_from_head(head: &str) -> String {
    extract_first_prompt_from_head_with(head, SummaryDialect::Python)
}

pub(crate) fn extract_first_prompt_from_head_with(head: &str, dialect: SummaryDialect) -> String {
    let mut command_fallback: Option<String> = None;
    for line in head.split('\n') {
        if !(line.contains("\"type\":\"user\"") || line.contains("\"type\": \"user\"")) {
            continue;
        }
        let skip = line.contains("\"tool_result\"")
            || line.contains("\"isMeta\":true")
            || line.contains("\"isMeta\": true")
            || line.contains("\"isCompactSummary\":true")
            || line.contains("\"isCompactSummary\": true");
        if skip {
            continue;
        }
        let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(prompt) = entry_first_prompt(dialect, &entry, &mut command_fallback) {
            return prompt;
        }
    }
    command_fallback.unwrap_or_default()
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

pub(crate) fn parse_session_info_from_lite(
    session_id: &str,
    lite: &LiteSessionFile,
    project_path: Option<&str>,
) -> Option<SdkSessionInfo> {
    let (head, tail) = (lite.head.as_str(), lite.tail.as_str());
    let first_line = head.split('\n').next().unwrap_or(head);
    if first_line.contains("\"isSidechain\":true") || first_line.contains("\"isSidechain\": true") {
        return None;
    }
    let custom_title = non_empty(extract_last_json_string_field(tail, "customTitle"))
        .or_else(|| non_empty(extract_last_json_string_field(head, "customTitle")))
        .or_else(|| non_empty(extract_last_json_string_field(tail, "aiTitle")))
        .or_else(|| non_empty(extract_last_json_string_field(head, "aiTitle")));
    let first_prompt = non_empty(Some(extract_first_prompt_from_head(head)));
    let summary = custom_title
        .clone()
        .or_else(|| non_empty(extract_last_json_string_field(tail, "lastPrompt")))
        .or_else(|| non_empty(extract_last_json_string_field(tail, "summary")))
        .or_else(|| first_prompt.clone())?;

    let git_branch = non_empty(extract_last_json_string_field(tail, "gitBranch"))
        .or_else(|| non_empty(extract_json_string_field(head, "gitBranch")));
    let cwd = non_empty(extract_json_string_field(head, "cwd")).or_else(|| project_path.map(str::to_owned));
    let tag = tail
        .split('\n')
        .rev()
        .find(|line| line.starts_with("{\"type\":\"tag\""))
        .and_then(|line| non_empty(extract_last_json_string_field(line, "tag")));
    let created_at = extract_json_string_field(head, "timestamp")
        .filter(|timestamp| !timestamp.is_empty())
        .and_then(|timestamp| iso_to_epoch_ms(SummaryDialect::Python, &timestamp));

    Some(SdkSessionInfo {
        session_id: SessionId::from(session_id),
        summary,
        last_modified: lite.mtime,
        file_size: Some(lite.size),
        custom_title,
        first_prompt,
        git_branch,
        cwd,
        tag,
        created_at,
    })
}

/// Runs `git worktree list --porcelain` in `cwd` with upstream's five second limit. Empty when git is missing or
/// `cwd` is not in a repository.
pub(crate) fn worktree_paths(cwd: &str) -> Vec<String> {
    let Ok(mut child) = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=",
            "worktree",
            "list",
            "--porcelain",
        ])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let Some(mut stdout) = child.stdout.take() else {
        return Vec::new();
    };
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stdout.read_to_string(&mut output);
        output
    });
    let deadline = SystemTime::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if SystemTime::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let Ok(output) = reader.join() else {
        return Vec::new();
    };
    if !status.is_some_and(|status| status.success()) || output.is_empty() {
        return Vec::new();
    }
    output
        .split('\n')
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(|path| path.nfc().collect())
        .collect()
}

/// `agent-<id>.jsonl` -> `agent-<id>.meta.json`, in the same directory.
pub(crate) fn agent_metadata_sidecar_path(transcript: &Path) -> PathBuf {
    let name = transcript
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.strip_suffix(".jsonl").unwrap_or(&name);
    transcript.with_file_name(format!("{stem}.meta.json"))
}

/// Reads the sidecar, treating a missing, unreadable, invalid or non-object file as absent.
pub(crate) fn read_agent_metadata_sidecar(transcript: &Path) -> Option<JsonObject> {
    let text = std::fs::read_to_string(agent_metadata_sidecar_path(transcript)).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(meta) => Some(meta),
        _ => None,
    }
}

/// Recursively collects `agent-*.jsonl` files, directory entries sorted by name at every level.
pub(crate) fn collect_agent_files(base: &Path) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path) in entries {
            if path.is_file()
                && let Some(id) = name.strip_prefix("agent-").and_then(|rest| rest.strip_suffix(".jsonl"))
            {
                out.push((id.to_owned(), path));
            } else if path.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut files = Vec::new();
    walk(base, &mut files);
    files
}
