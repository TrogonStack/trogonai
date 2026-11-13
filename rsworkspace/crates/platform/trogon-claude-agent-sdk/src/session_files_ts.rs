//! TypeScript SDK 0.3.323 session metadata and listing algorithms over the config directory.
//!
//! The Python SDK reads the same files with different rules, so [`SessionFiles`] keeps both and selects them with
//! [`SummaryDialect`], defaulting to TypeScript. This module is the TypeScript side: line oriented metadata
//! lookups that parse whole lines (`YA`), continued-in chains (`$z`), custom-title sidecars (`sc`), worktree
//! ownership checks (`c0`/`l0`) and the paged listing pipeline (`Gz`).

use crate::constants::{
    CONTINUED_MARK, CONTINUED_SCAN_CHUNK, CONTINUED_SCAN_LIMIT, LITE_READ_BUF_SIZE, MAX_SANITIZED_LENGTH, PAGE_WINDOW,
    PARENT_MARK,
};
use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use serde_json::Value;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{
    ListSessionsOptions, SdkSessionInfo, SessionFiles, SessionId, SummaryDialect,
    session_files::{
        LiteSessionFile, extract_first_prompt_from_head_with, extract_json_string_field, read_session_lite, string_end,
        unescape_json_string, valid_uuid, worktree_paths,
    },
    session_probe_ts::{blocks_probe, is_network_or_device_path},
    session_project_key::{canonicalize_lenient_in, hyphenate, platform_nfc, project_name_ts, sanitize_path},
    session_prompt::{entry_first_prompt, iso_to_epoch_ms, js_trim},
};

enum Parsed {
    Text(String),
    Missing,
    Invalid,
}

fn parse_typed(line: &str, key: &str, ty: Option<&str>) -> Parsed {
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(object)) => {
            if ty.is_none_or(|ty| object.get("type").and_then(Value::as_str) == Some(ty))
                && let Some(Value::String(text)) = object.get(key)
            {
                return Parsed::Text(text.clone());
            }
            Parsed::Missing
        }
        Ok(_) => Parsed::Missing,
        Err(_) => Parsed::Invalid,
    }
}

/// Upstream `YA`: the newest line, scanning backwards, that parses as JSON and holds a string `key` (of a `ty` entry
/// when given). With `fallback`, a line that fails to parse is retried from its last `{"type":"<ty>"` so a window
/// that starts inside a line still yields the entry.
pub(crate) fn last_typed_string(text: &str, key: &str, ty: Option<&str>, fallback: bool) -> Option<String> {
    let patterns: Option<Vec<String>> = ty.map(|ty| {
        let mut patterns = vec![format!("\"type\":\"{ty}\"")];
        if fallback {
            patterns.push(format!("\"type\": \"{ty}\""));
        }
        patterns
    });
    let needle = format!("\"{key}\":");
    let mut end = text.len();
    while end > 0 {
        let boundary = text[..end].rfind('\n');
        let line = &text[boundary.map_or(0, |index| index + 1)..end];
        end = boundary.unwrap_or(0);
        let typed = patterns
            .as_ref()
            .is_none_or(|patterns| patterns.iter().any(|pattern| line.contains(pattern.as_str())));
        if line.contains(&needle) && typed {
            let mut parsed = parse_typed(line, key, ty);
            if matches!(parsed, Parsed::Invalid)
                && fallback
                && let Some(first) = patterns.as_ref().and_then(|patterns| patterns.first())
            {
                parsed = match line.rfind(&format!("{{{first}")) {
                    Some(start)
                        if start > 0
                            && !line[..start]
                                .chars()
                                .next_back()
                                .is_some_and(|before| matches!(before, ':' | ',' | '[')) =>
                    {
                        parse_typed(&line[start..], key, ty)
                    }
                    _ => Parsed::Invalid,
                };
            }
            if let Parsed::Text(value) = parsed {
                return Some(value);
            }
        }
        if boundary.is_none() {
            break;
        }
    }
    None
}

/// Upstream `qA`: the string value with the highest offset over both key spellings.
pub(crate) fn latest_json_string_field(text: &str, key: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for pattern in [format!("\"{key}\":\""), format!("\"{key}\": \"")] {
        let mut from = 0;
        while let Some(found) = text.get(from..).and_then(|rest| rest.find(&pattern)) {
            let at = from + found;
            let value_start = at + pattern.len();
            let (end, stopped) = string_end(text, value_start);
            if let Some(end) = end
                && best.as_ref().is_none_or(|(seen, _)| at > *seen)
            {
                best = Some((at, unescape_json_string(&text[value_start..end])));
            }
            from = stopped + 1;
        }
    }
    best.map(|(_, value)| value)
}

/// Upstream `ZA`: the first line holding a parseable string `key`.
fn first_line_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    text.split('\n').filter(|line| line.contains(&needle)).find_map(|line| {
        match serde_json::from_str::<Value>(line).ok()? {
            Value::Object(object) => object.get(key).and_then(Value::as_str).map(str::to_owned),
            _ => None,
        }
    })
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

/// The working directory a session was relocated to, else its first recorded `cwd`.
fn relocated_cwd(lite: &LiteSessionFile) -> Option<String> {
    last_typed_string(&lite.tail, "relocatedCwd", Some("relocated"), false)
        .or_else(|| first_line_string(&lite.head, "cwd"))
}

/// Upstream `o0`: a prompt-less session that opened with an image or a document.
fn media_summary(head: &str) -> &'static str {
    for line in head.split('\n') {
        if !(line.contains("\"type\":\"user\"") || line.contains("\"type\": \"user\"")) {
            continue;
        }
        if line.contains("\"tool_result\"") || line.contains("\"isMeta\":true") || line.contains("\"isMeta\": true") {
            continue;
        }
        if line.contains("\"type\":\"image\"") || line.contains("\"type\": \"image\"") {
            return "Image";
        }
        if line.contains("\"type\":\"document\"") || line.contains("\"type\": \"document\"") {
            return "Document";
        }
    }
    ""
}

/// Upstream `Lz`: collapse control and format characters, drop the rest of C0 and C1, cap at 200 characters.
fn clean_title(raw: &str) -> Option<String> {
    let mut flattened = String::new();
    let mut in_run = false;
    for ch in js_trim(raw).chars() {
        let invisible = matches!(
            get_general_category(ch),
            GeneralCategory::Control | GeneralCategory::Format
        ) || matches!(ch, '\u{2028}' | '\u{2029}');
        if invisible {
            if !in_run {
                flattened.push(' ');
            }
            in_run = true;
        } else {
            flattened.push(ch);
            in_run = false;
        }
    }
    let capped: String = flattened
        .chars()
        .filter(|ch| !matches!(u32::from(*ch), 0..=0x1f | 0x7f..=0x9f))
        .take(200)
        .collect();
    let title = js_trim(&capped);
    (!title.is_empty()).then(|| title.to_owned())
}

/// Upstream `sc`: `<project dir>/<session id>/custom-title.json`, a sidecar written next to the transcript.
pub(crate) fn sidecar_title(transcript: &Path, session_id: &str) -> Option<String> {
    let path = transcript.parent()?.join(session_id).join("custom-title.json");
    let text = std::fs::read_to_string(path).ok()?;
    let Value::Object(object) = serde_json::from_str::<Value>(&text).ok()? else {
        return None;
    };
    clean_title(object.get("customTitle")?.as_str()?)
}

/// Upstream `ic`: session metadata from a lite read, using `sidecar` as the title when the tail has none.
pub(crate) fn session_info_from_lite(
    session_id: &str,
    lite: &LiteSessionFile,
    project_path: Option<&str>,
    sidecar: Option<String>,
) -> Option<SdkSessionInfo> {
    let (head, tail) = (lite.head.as_str(), lite.tail.as_str());
    let first_line = head.split('\n').next().unwrap_or(head);
    if first_line.contains("\"isSidechain\":true") || first_line.contains("\"isSidechain\": true") {
        return None;
    }
    let titled = |text: &str, ty: &str, key: &str| non_empty(last_typed_string(text, key, Some(ty), true));
    let custom_title = titled(tail, "custom-title", "customTitle")
        .or_else(|| non_empty(sidecar))
        .or_else(|| titled(head, "custom-title", "customTitle"))
        .or_else(|| titled(tail, "ai-title", "aiTitle"))
        .or_else(|| titled(head, "ai-title", "aiTitle"));
    let first_prompt = non_empty(Some(extract_first_prompt_from_head_with(
        head,
        SummaryDialect::TypeScript,
    )));
    let created_at = non_empty(extract_json_string_field(head, "timestamp"))
        .and_then(|timestamp| iso_to_epoch_ms(SummaryDialect::TypeScript, &timestamp));
    let summary = custom_title
        .clone()
        .or_else(|| non_empty(last_typed_string(tail, "lastPrompt", None, false)))
        .or_else(|| non_empty(last_typed_string(tail, "summary", None, false)))
        .or_else(|| first_prompt.clone())
        .or_else(|| non_empty(Some(media_summary(head).to_owned())))?;
    let git_branch = non_empty(latest_json_string_field(tail, "gitBranch"))
        .or_else(|| non_empty(extract_json_string_field(head, "gitBranch")));
    let cwd = non_empty(last_typed_string(tail, "relocatedCwd", Some("relocated"), false))
        .or_else(|| non_empty(extract_json_string_field(head, "cwd")))
        .or_else(|| non_empty(project_path.map(str::to_owned)));
    let tag = titled(tail, "tag", "tag");
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

/// Upstream `n0`: sessions started by the SDKs or a daemon rather than a person.
pub(crate) fn is_programmatic(head: &str, tail: &str) -> bool {
    let entrypoint =
        extract_json_string_field(head, "entrypoint").or_else(|| latest_json_string_field(tail, "entrypoint"));
    if entrypoint
        .as_deref()
        .is_some_and(|value| matches!(value, "sdk-cli" | "sdk-ts" | "sdk-py"))
    {
        return true;
    }
    let first = head.split('\n').find(|line| line.contains(PARENT_MARK)).unwrap_or(head);
    matches!(
        extract_json_string_field(first, "sessionKind").as_deref(),
        Some("daemon" | "daemon-worker")
    )
}

/// Upstream `CLe`: whether a newer entry shows the session carried on after its `continued-in` marker.
fn shows_progress(value: &Value) -> bool {
    if let Value::Object(object) = value {
        let assistant = object.get("type").and_then(Value::as_str) == Some("assistant");
        let flag = object.get("isApiErrorMessage");
        let message = object.get("message");
        let stop = message
            .and_then(Value::as_object)
            .map(|message| message.get("stop_reason"));
        let shaped = assistant
            && flag.is_none_or(Value::is_boolean)
            && message.is_none_or(|message| {
                message.as_object().is_some_and(|message| {
                    message
                        .get("stop_reason")
                        .is_none_or(|stop| stop.is_null() || stop.is_string())
                })
            });
        if shaped {
            return flag.and_then(Value::as_bool) != Some(true) && matches!(stop, Some(Some(Value::String(_))));
        }
        let mut fallback = None;
        return entry_first_prompt(SummaryDialect::TypeScript, object, &mut fallback).is_some();
    }
    false
}

/// Upstream `$z`: the session a transcript was continued into, when its newest entry says so.
pub(crate) fn continued_in(tail: &str) -> Option<String> {
    if !tail.contains(CONTINUED_MARK) {
        return None;
    }
    let mut end = tail.len();
    while end > 0 {
        let boundary = tail[..end].rfind('\n');
        let line = &tail[boundary.map_or(0, |index| index + 1)..end];
        end = boundary.unwrap_or(0);
        let marker = line.contains(CONTINUED_MARK);
        let message = line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"");
        if (marker || message)
            && let Ok(value) = serde_json::from_str::<Value>(line)
        {
            if marker
                && let Some(object) = value.as_object()
                && object.get("type").and_then(Value::as_str) == Some("continued-in")
                && let Some(Value::String(id)) = object.get("continuedInSessionId")
            {
                return valid_uuid(id).map(str::to_owned);
            }
            if message && shows_progress(&value) {
                return None;
            }
        }
        if boundary.is_none() {
            break;
        }
    }
    None
}

/// Upstream `zz`/`Bz`: the continued-into transcript exists and holds a conversation (a `parentUuid` somewhere in
/// its first 16 MiB).
fn continuation_has_conversation(dir: &Path, id: &str) -> bool {
    let path = dir.join(format!("{id}.jsonl"));
    let Some(lite) = read_session_lite(&path) else {
        return false;
    };
    if lite.head.contains(PARENT_MARK) || lite.tail.contains(PARENT_MARK) {
        return true;
    }
    if lite.size <= LITE_READ_BUF_SIZE {
        return false;
    }
    let Ok(mut file) = File::open(&path) else {
        return false;
    };
    let mut carry: Vec<u8> = Vec::new();
    let mut scanned = 0_u64;
    let mut chunk = vec![0_u8; CONTINUED_SCAN_CHUNK];
    while scanned < CONTINUED_SCAN_LIMIT {
        let Ok(read) = file.read(&mut chunk) else {
            return false;
        };
        if read == 0 {
            return false;
        }
        let mut window = carry.clone();
        window.extend_from_slice(&chunk[..read]);
        if window
            .windows(PARENT_MARK.len())
            .any(|candidate| candidate == PARENT_MARK.as_bytes())
        {
            return true;
        }
        let keep = PARENT_MARK.len().min(window.len());
        carry = window[window.len() - keep..].to_vec();
        scanned += read as u64;
    }
    true
}

fn case_insensitive_platform() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

fn unified(path: &str, fold: bool) -> String {
    let slashed = tt(path).replace('\\', "/");
    if fold { slashed.to_lowercase() } else { slashed }
}

/// Upstream `Vl` followed by the platform's case rule.
fn name_key(path: &str, fold: bool) -> String {
    let name = hyphenate(&tt(path), SummaryDialect::TypeScript);
    if fold { name.to_lowercase() } else { name }
}

/// Upstream `qU`: the two paths sanitize to the same project name yet are different paths.
fn colliding_names(a: &str, b: &str, fold: bool) -> bool {
    name_key(a, fold) == name_key(b, fold) && unified(a, fold) != unified(b, fold)
}

/// Upstream `c0`: `path` is one of the worktrees (or inside one).
fn within_worktrees(path: &str, worktrees: Option<&[String]>, fold: bool) -> bool {
    let Some(worktrees) = worktrees else {
        return false;
    };
    let path = unified(path, fold);
    worktrees.iter().any(|worktree| {
        let root = unified(worktree, fold);
        path == root
            || path.starts_with(&if root.ends_with('/') {
                root.clone()
            } else {
                format!("{root}/")
            })
    })
}

/// Upstream `tt`.
fn tt(path: &str) -> String {
    platform_nfc(path, SummaryDialect::TypeScript)
}

/// Upstream `l0`: `cwd` is a different directory whose project name collides with `project_path`'s, so the session
/// belongs to another project that merely shares the sanitized directory name.
fn foreign_lookalike(cwd: &str, project_path: &str) -> bool {
    let fold = case_insensitive_platform();
    if !colliding_names(cwd, project_path, fold) {
        return false;
    }
    if is_network_or_device_path(cwd) || is_network_or_device_path(project_path) {
        return false;
    }
    if blocks_probe(cwd) || blocks_probe(project_path) {
        return false;
    }
    let Ok(real) = std::fs::canonicalize(cwd) else {
        return false;
    };
    let real = tt(&real.to_string_lossy());
    colliding_names(&real, &resolved_project_path(project_path), fold)
}

/// Upstream `is`: the realpath of `path`, or `path` itself when it cannot be resolved.
fn resolved_project_path(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(real) => tt(&real.to_string_lossy()),
        Err(_) => tt(path),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    session_id: String,
    path: PathBuf,
    mtime: u64,
    project_path: Option<String>,
    own_worktrees: Option<Vec<String>>,
}

/// Upstream `Kz`: reads one candidate and applies every exclusion the listing has.
fn candidate_info(candidate: &Candidate, include_programmatic: bool) -> Option<SdkSessionInfo> {
    let lite = read_session_lite(&candidate.path)?;
    if !include_programmatic && is_programmatic(&lite.head, &lite.tail) {
        return None;
    }
    if let Some(target) = continued_in(&lite.tail)
        && let Some(dir) = candidate.path.parent()
        && continuation_has_conversation(dir, &target)
    {
        return None;
    }
    let sidecar = if last_typed_string(&lite.tail, "customTitle", Some("custom-title"), true).is_none() {
        sidecar_title(&candidate.path, &candidate.session_id)
    } else {
        None
    };
    let mut info = session_info_from_lite(&candidate.session_id, &lite, candidate.project_path.as_deref(), sidecar)?;
    if let (Some(cwd), Some(project)) = (relocated_cwd(&lite), candidate.project_path.as_deref())
        && !within_worktrees(&cwd, candidate.own_worktrees.as_deref(), case_insensitive_platform())
        && foreign_lookalike(&cwd, project)
    {
        return None;
    }
    if candidate.mtime != 0 {
        info.last_modified = candidate.mtime;
    }
    Some(info)
}

/// Upstream `kLe`: every candidate, the newest copy of each session first.
fn collect_all(candidates: &[Candidate], include_programmatic: bool) -> Vec<SdkSessionInfo> {
    let mut by_id: Vec<SdkSessionInfo> = Vec::new();
    for info in candidates
        .iter()
        .filter_map(|candidate| candidate_info(candidate, include_programmatic))
    {
        match by_id.iter_mut().find(|seen| seen.session_id == info.session_id) {
            Some(seen) if info.last_modified > seen.last_modified => *seen = info,
            Some(_) => {}
            None => by_id.push(info),
        }
    }
    by_id.sort_by(|a, b| {
        b.last_modified
            .cmp(&a.last_modified)
            .then_with(|| b.session_id.as_str().cmp(a.session_id.as_str()))
    });
    by_id
}

/// Upstream `xLe`: candidates ordered by file time, read in windows until the page is full.
fn collect_page(
    mut candidates: Vec<Candidate>,
    limit: Option<usize>,
    offset: usize,
    include_programmatic: bool,
) -> Vec<SdkSessionInfo> {
    candidates.sort_by(|a, b| {
        b.mtime
            .cmp(&a.mtime)
            .then_with(|| b.session_id.as_str().cmp(a.session_id.as_str()))
    });
    let cap = limit.filter(|limit| *limit > 0).unwrap_or(usize::MAX);
    let mut page = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut skipped = 0;
    for window in candidates.chunks(PAGE_WINDOW) {
        for candidate in window {
            if page.len() >= cap {
                return page;
            }
            let Some(info) = candidate_info(candidate, include_programmatic) else {
                continue;
            };
            if !seen.insert(info.session_id.as_str().to_owned()) {
                continue;
            }
            if skipped < offset {
                skipped += 1;
                continue;
            }
            page.push(info);
        }
        if page.len() >= cap {
            break;
        }
    }
    page
}

fn fold_name(name: &str) -> String {
    if cfg!(windows) {
        name.to_lowercase()
    } else {
        name.to_owned()
    }
}

/// Upstream `Yl`/`u0`: some transcript in `dir` was recorded in a directory that sanitizes like `path`.
fn holds_session_of(dir: &Path, path: &str) -> bool {
    let wanted = fold_name(&hyphenate(path, SummaryDialect::TypeScript));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let file = entry.path();
        if !(file.is_file() && entry.file_name().to_string_lossy().ends_with(".jsonl")) {
            return false;
        }
        read_session_lite(&file)
            .and_then(|lite| relocated_cwd(&lite))
            .is_some_and(|cwd| fold_name(&hyphenate(&tt(&cwd), SummaryDialect::TypeScript)) == wanted)
    })
}

fn candidates_in(
    dir: &Path,
    stat: bool,
    project_path: Option<&str>,
    own_worktrees: Option<&[String]>,
) -> Vec<Candidate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let session_id = valid_uuid(name.strip_suffix(".jsonl")?)?.to_owned();
            let path = dir.join(&name);
            let mtime = if stat {
                let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|after| u64::try_from(after.as_millis()).ok())
                    .unwrap_or(0)
            } else {
                0
            };
            Some(Candidate {
                session_id,
                path,
                mtime,
                project_path: project_path.map(str::to_owned),
                own_worktrees: own_worktrees.map(<[String]>::to_vec),
            })
        })
        .collect()
}

impl SessionFiles {
    /// Upstream `dr`: the project directories holding sessions of `path`. Besides the exact directory, a name that
    /// was cut at 200 characters may have been hashed differently by another client, so directories with the same
    /// prefix count when one of their transcripts was recorded in `path`.
    pub(crate) fn project_dirs_for_ts(&self, path: &str) -> Vec<PathBuf> {
        let sanitized = project_name_ts(path);
        let exact = self.projects_dir().join(&sanitized);
        let mut found = Vec::new();
        if exact.is_dir() {
            found.push(exact.clone());
        }
        let hashed = sanitize_path(path, SummaryDialect::TypeScript);
        if hashed != sanitized {
            let other = self.projects_dir().join(&hashed);
            if other.is_dir() {
                found.push(other);
            }
            return found;
        }
        if sanitized.len() <= MAX_SANITIZED_LENGTH {
            return found;
        }
        let prefix = fold_name(&format!("{}-", &sanitized[..MAX_SANITIZED_LENGTH]));
        let exact_name = fold_name(&exact.to_string_lossy());
        let Ok(entries) = std::fs::read_dir(self.projects_dir()) else {
            return found;
        };
        let mut matches: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|dir| {
                dir.is_dir()
                    && dir
                        .file_name()
                        .is_some_and(|name| fold_name(&name.to_string_lossy()).starts_with(&prefix))
            })
            .collect();
        matches.sort();
        for dir in matches {
            if fold_name(&dir.to_string_lossy()) != exact_name && holds_session_of(&dir, path) {
                found.push(dir);
            }
        }
        found
    }

    fn unique_project_dirs(&self, path: &str) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        self.project_dirs_for_ts(path)
            .into_iter()
            .filter(|dir| seen.insert(fold_name(&dir.to_string_lossy())))
            .collect()
    }

    /// Upstream `PLe`/`jz`: candidates of one project directory and, optionally, its sibling worktrees.
    fn candidates_for_project(&self, directory: &Path, include_worktrees: bool, stat: bool) -> Vec<Candidate> {
        let canonical = canonicalize_lenient_in(directory, SummaryDialect::TypeScript);
        let worktrees = if include_worktrees {
            worktree_paths(&canonical, SummaryDialect::TypeScript)
        } else {
            Vec::new()
        };
        if worktrees.len() <= 1 {
            return self
                .unique_project_dirs(&canonical)
                .iter()
                .flat_map(|dir| candidates_in(dir, stat, Some(&canonical), None))
                .collect();
        }
        let own: Vec<String> = std::iter::once(canonical.clone())
            .chain(worktrees.iter().cloned())
            .collect();
        let mut named: Vec<(String, String, Option<String>)> = worktrees
            .iter()
            .map(|path| {
                let exact = fold_name(&project_name_ts(path));
                let truncated = (exact.len() > MAX_SANITIZED_LENGTH).then(|| exact[..MAX_SANITIZED_LENGTH].to_owned());
                (path.clone(), exact, truncated)
            })
            .collect();
        named.sort_by_key(|(_, exact, _)| std::cmp::Reverse(exact.len()));

        let mut subdirs: Vec<String> = self
            .project_dirs()
            .iter()
            .filter_map(|dir| dir.file_name().map(|name| name.to_string_lossy().into_owned()))
            .collect();
        subdirs.sort();
        let mut seen: HashSet<String> = HashSet::new();
        let mut candidates = Vec::new();
        for dir in self.project_dirs_for_ts(&canonical) {
            let name = dir
                .file_name()
                .map(|name| fold_name(&name.to_string_lossy()))
                .unwrap_or_default();
            if seen.insert(name) {
                candidates.extend(candidates_in(&dir, stat, Some(&canonical), Some(&own)));
            }
        }
        for name in subdirs {
            let folded = fold_name(&name);
            if seen.contains(&folded) {
                continue;
            }
            let dir = self.projects_dir().join(&name);
            for (worktree, exact, truncated) in &named {
                let hit = folded == *exact
                    || truncated.as_ref().is_some_and(|prefix| {
                        folded.starts_with(&format!("{prefix}-")) && holds_session_of(&dir, worktree)
                    });
                if hit {
                    seen.insert(folded.clone());
                    candidates.extend(candidates_in(&dir, stat, Some(worktree), Some(&own)));
                    break;
                }
            }
        }
        candidates
    }

    /// Upstream `DLe`: every project directory.
    fn candidates_for_all(&self, stat: bool) -> Vec<Candidate> {
        let mut dirs = self.project_dirs();
        dirs.sort();
        dirs.iter()
            .flat_map(|dir| candidates_in(dir, stat, None, None))
            .collect()
    }

    /// Upstream `Gz` (`listSessions`) over the TypeScript rules.
    pub(crate) fn list_sessions_ts(&self, options: &ListSessionsOptions) -> Vec<SdkSessionInfo> {
        let paged = options.limit.is_some_and(|limit| limit > 0) || options.offset > 0;
        let candidates = match options
            .directory
            .as_deref()
            .filter(|directory| !directory.as_os_str().is_empty())
        {
            Some(directory) => self.candidates_for_project(directory, options.include_worktrees, paged),
            None => self.candidates_for_all(paged),
        };
        if paged {
            collect_page(candidates, options.limit, options.offset, options.include_programmatic)
        } else {
            collect_all(&candidates, options.include_programmatic)
        }
    }

    /// Upstream `ei`: the transcript of a session and the project path it was found under.
    pub(crate) fn resolve_session_ts(
        &self,
        session_id: &str,
        directory: Option<&Path>,
    ) -> Option<(PathBuf, Option<String>)> {
        let file_name = format!("{session_id}.jsonl");
        let non_empty_file = |dir: &Path| {
            let path = dir.join(&file_name);
            std::fs::metadata(&path)
                .ok()
                .filter(|metadata| metadata.len() > 0)
                .map(|_| path)
        };
        let Some(directory) = directory.filter(|directory| !directory.as_os_str().is_empty()) else {
            let mut dirs = self.project_dirs();
            dirs.sort();
            return dirs.iter().find_map(|dir| non_empty_file(dir)).map(|path| (path, None));
        };
        let canonical = canonicalize_lenient_in(directory, SummaryDialect::TypeScript);
        for dir in self.unique_project_dirs(&canonical) {
            if let Some(path) = non_empty_file(&dir) {
                return Some((path, Some(canonical)));
            }
        }
        for worktree in worktree_paths(&canonical, SummaryDialect::TypeScript) {
            if worktree == canonical {
                continue;
            }
            for dir in self.project_dirs_for_ts(&worktree) {
                if let Some(path) = non_empty_file(&dir) {
                    return Some((path, Some(worktree)));
                }
            }
        }
        None
    }

    /// Upstream `Wz` (`getSessionInfo`) over the TypeScript rules.
    pub(crate) fn get_session_info_ts(&self, session_id: &str, directory: Option<&Path>) -> Option<SdkSessionInfo> {
        let session_id = valid_uuid(session_id)?;
        let (path, project_path) = self.resolve_session_ts(session_id, directory)?;
        let lite = read_session_lite(&path)?;
        let sidecar = if last_typed_string(&lite.tail, "customTitle", Some("custom-title"), true).is_none() {
            sidecar_title(&path, session_id)
        } else {
            None
        };
        session_info_from_lite(session_id, &lite, project_path.as_deref(), sidecar)
    }
}
