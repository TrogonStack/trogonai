//! Filesystem entries as the host spells them: `~`, relative paths and
//! symlinks, resolved against the host's working directory and home.

use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{FsPathEntry, GlobPattern, LiteralPath};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::constants::{GLOB_CHARS, WINDOWS_GLOB_CHARS};
use super::fs_path::{self, FsPathError};

#[cfg(windows)]
use super::windows_paths;

/// On Windows only `*` and `?` are glob characters, so `[` and `]` stay part
/// of a name.
fn glob_chars(windows: bool) -> &'static [char] {
    if windows { &WINDOWS_GLOB_CHARS } else { &GLOB_CHARS }
}

/// Which way a list cuts. A deny takes every reading of an entry that may
/// hold, since one more can only deny more; an allow takes one only for a path
/// that is there exactly as spelled.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum ListKind {
    Allow,
    Deny,
}

enum Reading {
    Name(String),
    Beneath { anchor: String, tail: String },
}

/// What one settings entry came to: the entries it stands for, and what the
/// operator should be told about how it was read.
#[derive(Debug, Default)]
pub(super) struct ResolvedEntry {
    pub entries: Vec<FsPathEntry>,
    pub warnings: Vec<String>,
}

impl ResolvedEntry {
    fn push(&mut self, entry: FsPathEntry) {
        if !self.entries.contains(&entry) {
            self.entries.push(entry);
        }
    }

    fn note_textual_brackets(&mut self, pattern: &str) {
        let brackets = fs_path::textual_brackets(pattern);
        if !brackets.is_empty() {
            self.warnings.push(format!(
                "The brackets in {} spell no set of characters, so they are read as text: {pattern}",
                brackets.join(", ")
            ));
        }
    }

    fn note_unfolded_parent(&mut self, path: &str) {
        if path.split('/').any(|part| part == "..") {
            self.warnings.push(format!(
                "\"{path}\" could not be resolved and still contains a \"..\" component, so a rule spelled this way may not match."
            ));
        }
    }
}

/// Where `~` and relative entries point, and whether symlinks are followed.
#[derive(Debug, Clone)]
pub struct HostPaths {
    cwd: PathBuf,
    home: Option<PathBuf>,
}

impl HostPaths {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            home: None,
        }
    }

    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// The working directory and `HOME` of this process.
    pub fn from_process() -> std::io::Result<Self> {
        Self::from_env(&SystemEnv)
    }

    /// The working directory of this process and the home `env` names.
    pub(super) fn from_env(env: &impl ReadEnv) -> std::io::Result<Self> {
        let cwd = std::env::current_dir()?;
        Ok(Self {
            cwd,
            home: home_var(env).map(PathBuf::from),
        })
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(super) fn resolve(&self, text: &str, literal: bool, kind: ListKind) -> Result<ResolvedEntry, FsPathError> {
        if text.is_empty() {
            return Err(FsPathError::Empty);
        }
        let mut resolved = ResolvedEntry::default();
        let directory = names_a_directory(text, kind);
        if literal {
            let collapsed = collapse_interior(&self.absolute(text));
            resolved.push(self.name_entry(&collapsed, directory)?);
            return Ok(resolved);
        }
        let stripped = remove_trailing_glob_suffix(text);
        if !has_glob_chars(&stripped) {
            let collapsed = collapse_interior(&self.absolute(&stripped));
            resolved.push(self.name_entry(&collapsed, directory)?);
            return Ok(resolved);
        }
        let collapsed = collapse_interior(&self.absolute(&stripped));
        resolved.note_textual_brackets(&collapsed);
        match fs_path::glob(&collapsed) {
            Err(err) if err.is_not_a_glob() && kind == ListKind::Allow => {
                let resolved_pattern = resolve_static_prefix(&collapsed);
                resolved.note_unfolded_parent(&resolved_pattern);
                resolved.push(fs_path::literal(&resolved_pattern).or_else(|_| fs_path::literal(&collapsed))?);
            }
            Err(err) if err.is_not_a_glob() => {
                resolved.push(self.name_entry(&collapsed, false)?);
            }
            Ok(whole) => {
                let resolved_pattern = resolve_static_prefix(&collapsed);
                resolved.note_unfolded_parent(&resolved_pattern);
                resolved.push(fs_path::glob(&resolved_pattern).unwrap_or(whole));
            }
            Err(err) => return Err(err),
        }
        for reading in self.other_readings(&collapsed, kind) {
            match reading {
                Reading::Name(path) => {
                    resolved.note_unfolded_parent(&path);
                    resolved.push(fs_path::literal(&path)?);
                }
                Reading::Beneath { anchor, tail } => {
                    resolved.note_unfolded_parent(&anchor);
                    let anchor = LiteralPath::new(&anchor)?;
                    resolved.push(FsPathEntry::Glob {
                        pattern: GlobPattern::anchored(anchor, tail)?,
                    });
                }
            }
        }
        Ok(resolved)
    }

    fn name_entry(&self, collapsed: &str, directory: bool) -> Result<FsPathEntry, FsPathError> {
        let name = real_path_within_bounds(&trim_trailing(collapsed));
        if directory {
            fs_path::literal_directory(&name)
        } else {
            fs_path::literal(&name)
        }
    }

    /// The readings an entry has besides the one its characters give it: the
    /// path it spells where that exists, and a pattern beneath each existing
    /// directory whose own name holds the characters. A deny takes every
    /// reading that may hold; an allow only those on disk exactly as spelled.
    fn other_readings(&self, spelled: &str, kind: ListKind) -> Vec<Reading> {
        let parts: Vec<&str> = spelled.split('/').collect();
        let glob_at: Vec<usize> = parts
            .iter()
            .enumerate()
            .filter(|(_, part)| has_glob_chars(part))
            .map(|(index, _)| index)
            .collect();
        let (Some(&first), Some(&last)) = (glob_at.first(), glob_at.last()) else {
            return Vec::new();
        };
        let prefix = |index: usize| -> String {
            let joined = parts[..=index].join("/");
            if joined.is_empty() { "/".to_string() } else { joined }
        };
        let is_there = |path: &str| match kind {
            ListKind::Deny => may_be_there(path),
            ListKind::Allow => is_there_unlinked(path),
        };
        let mut reach = first - 1;
        while reach < last && is_there(&prefix(reach + 1)) {
            reach += 1;
        }
        if reach < first {
            return Vec::new();
        }
        let mut readings = Vec::new();
        if reach == last && (kind == ListKind::Deny || rest_is_unlinked(last, parts.len() - 1, &prefix)) {
            readings.push(Reading::Name(real_path_within_bounds(&trim_trailing(spelled))));
        }
        for n in (0..glob_at.len().saturating_sub(1)).rev() {
            let at = glob_at[n];
            if at > reach {
                continue;
            }
            let next = glob_at[n + 1];
            let Some(end) = (at..=(next - 1).min(reach))
                .rev()
                .find(|end| is_directory_for(kind, &prefix(*end)))
            else {
                continue;
            };
            if kind == ListKind::Allow && !rest_is_unlinked(end, next - 1, &prefix) {
                continue;
            }
            let anchor_text = prefix(end);
            readings.push(Reading::Beneath {
                anchor: real_path_within_bounds(&anchor_text),
                tail: spelled[anchor_text.len()..].to_string(),
            });
        }
        readings
    }

    /// `text` as a single file or directory, never a pattern.
    pub(super) fn resolve_literal(&self, text: &str) -> Result<LiteralPath, FsPathError> {
        let resolved = self.resolve(text, true, ListKind::Allow)?;
        match resolved.entries.into_iter().next() {
            Some(FsPathEntry::Literal { path, .. }) => Ok(path),
            _ => Err(FsPathError::Empty),
        }
    }

    #[cfg(unix)]
    fn absolute(&self, text: &str) -> String {
        if text == "~" {
            return self.home_text();
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return format!("{}/{rest}", self.home_text());
        }
        if text.starts_with('/') {
            return text.to_string();
        }
        let joined = format!("{}/{text}", self.cwd.to_string_lossy());
        joined.trim_end_matches('/').to_string()
    }

    #[cfg(windows)]
    fn absolute(&self, text: &str) -> String {
        let expanded = expand_windows_env_refs(
            text,
            &self
                .home
                .as_ref()
                .map(|home| home.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        let text = expanded.as_str();
        let home = || self.home_text();
        if text == "~" {
            return home();
        }
        if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix(r"~\")) {
            return format!("{}/{}", home(), rest.replace('\\', "/"));
        }
        if let Some(path) = windows_paths::internal(text) {
            return path;
        }
        let joined = format!("{}/{}", self.cwd_text(), text.replace('\\', "/"));
        joined.trim_end_matches('/').to_string()
    }

    #[cfg(windows)]
    fn cwd_text(&self) -> String {
        windows_paths::internal(&self.cwd.to_string_lossy()).unwrap_or_else(|| self.cwd.to_string_lossy().into_owned())
    }

    #[cfg(unix)]
    fn home_text(&self) -> String {
        self.home
            .as_ref()
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    #[cfg(windows)]
    fn home_text(&self) -> String {
        self.home
            .as_ref()
            .map(|home| {
                windows_paths::internal(&home.to_string_lossy()).unwrap_or_else(|| home.to_string_lossy().into_owned())
            })
            .unwrap_or_default()
    }
}

/// `%USERPROFILE%`,
/// `%HOMEDRIVE%` and `%HOMEPATH%` read as the home directory, its drive and
/// the rest of it, whatever their case.
#[cfg(any(windows, test))]
fn expand_windows_env_refs(text: &str, home: &str) -> String {
    let bytes = home.as_bytes();
    let has_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let (drive, home_path) = if has_drive { home.split_at(2) } else { ("", home) };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let replacement = [
            ("%USERPROFILE%", home),
            ("%HOMEDRIVE%", drive),
            ("%HOMEPATH%", home_path),
        ]
        .into_iter()
        .find(|(name, _)| {
            tail.get(..name.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(name))
        });
        match replacement {
            Some((name, value)) => {
                out.push_str(value);
                rest = &tail[name.len()..];
            }
            None => {
                out.push('%');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A Windows deny keeps the trailing separator of a path that is not there, so
/// the placeholder created for it is a directory. Elsewhere the spelling is
/// dropped.
fn names_a_directory(text: &str, kind: ListKind) -> bool {
    cfg!(windows) && kind == ListKind::Deny && text.ends_with(['/', '\\'])
}

fn home_var(env: &impl ReadEnv) -> Option<std::ffi::OsString> {
    env.var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
}

fn collapse_interior(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_slash = false;
    for ch in text.chars() {
        if ch == '/' {
            if previous_slash {
                continue;
            }
            previous_slash = true;
        } else {
            previous_slash = false;
        }
        out.push(ch);
    }
    while let Some(index) = out.find("/./") {
        out.replace_range(index..index + 2, "");
    }
    out
}

fn trim_trailing(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let stripped = text.strip_suffix("/.").unwrap_or(text);
    let trimmed = stripped.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(super) fn has_glob_chars(text: &str) -> bool {
    contains_glob_chars(text, cfg!(windows))
}

fn contains_glob_chars(text: &str, windows: bool) -> bool {
    text.contains(glob_chars(windows))
}

pub(super) fn remove_trailing_glob_suffix(text: &str) -> String {
    match text.strip_suffix("/**") {
        Some("") => "/".to_string(),
        Some(rest) => rest.to_string(),
        None => text.to_string(),
    }
}

#[cfg(windows)]
fn is_absence(err: &std::io::Error) -> bool {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_PATH_NOT_FOUND: i32 = 3;
    const ERROR_INVALID_NAME: i32 = 123;
    const ERROR_FILENAME_EXCED_RANGE: i32 = 206;
    const ERROR_DIRECTORY: i32 = 267;
    err.raw_os_error().is_some_and(|raw| {
        matches!(
            raw,
            ERROR_FILE_NOT_FOUND
                | ERROR_PATH_NOT_FOUND
                | ERROR_INVALID_NAME
                | ERROR_FILENAME_EXCED_RANGE
                | ERROR_DIRECTORY
        )
    })
}

#[cfg(unix)]
fn is_absence(err: &std::io::Error) -> bool {
    err.raw_os_error().is_some_and(|raw| {
        matches!(
            nix::errno::Errno::from_raw(raw),
            nix::errno::Errno::ENOENT
                | nix::errno::Errno::ENOTDIR
                | nix::errno::Errno::ELOOP
                | nix::errno::Errno::ENAMETOOLONG
        )
    })
}

/// `path`, a policy path, as the host names it.
#[cfg(unix)]
fn fs_path(path: &str) -> PathBuf {
    PathBuf::from(path)
}

#[cfg(windows)]
fn fs_path(path: &str) -> PathBuf {
    PathBuf::from(windows_paths::native(path).unwrap_or_else(|| path.to_string()))
}

fn may_be_there(path: &str) -> bool {
    match std::fs::symlink_metadata(fs_path(path)) {
        Ok(_) => true,
        Err(err) => !is_absence(&err),
    }
}

fn is_there_unlinked(path: &str) -> bool {
    std::fs::symlink_metadata(fs_path(path)).is_ok_and(|meta| !meta.file_type().is_symlink())
}

fn is_directory_for(kind: ListKind, path: &str) -> bool {
    match kind {
        ListKind::Allow => std::fs::symlink_metadata(fs_path(path)).is_ok_and(|meta| meta.is_dir()),
        ListKind::Deny => match std::fs::metadata(fs_path(path)) {
            Ok(meta) => meta.is_dir(),
            Err(err) => !is_absence(&err),
        },
    }
}

fn rest_is_unlinked(from: usize, to: usize, prefix: &dyn Fn(usize) -> String) -> bool {
    for index in from + 1..=to {
        match std::fs::symlink_metadata(fs_path(&prefix(index))) {
            Ok(meta) if meta.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(err) => return is_absence(&err),
        }
    }
    true
}

fn resolve_static_prefix(pattern: &str) -> String {
    let prefix = pattern.split(glob_chars(cfg!(windows))).next().unwrap_or("");
    if prefix.is_empty() || prefix == "/" {
        return pattern.to_string();
    }
    let base = match prefix.strip_suffix('/') {
        Some(base) => base.to_string(),
        None => Path::new(prefix)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    if base.is_empty() {
        return pattern.to_string();
    }
    let real = real_path_within_bounds(&base);
    format!("{real}{}", &pattern[base.len()..])
}

fn real_path_within_bounds(text: &str) -> String {
    match std::fs::canonicalize(fs_path(text)) {
        Ok(real) => {
            let real = policy_spelling(&real);
            if outside_boundary(text, &real) {
                text.to_string()
            } else {
                real
            }
        }
        Err(_) => text.to_string(),
    }
}

#[cfg(unix)]
fn policy_spelling(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(windows)]
fn policy_spelling(path: &Path) -> String {
    let text = path.to_string_lossy();
    windows_paths::internal(&text).unwrap_or_else(|| text.into_owned())
}

fn outside_boundary(original: &str, resolved: &str) -> bool {
    if resolved == original {
        return false;
    }
    let canonical = ["/tmp/", "/var/"]
        .iter()
        .any(|prefix| original.starts_with(prefix))
        .then(|| format!("/private{original}"));
    if canonical.as_deref() == Some(resolved) {
        return false;
    }
    if resolved == "/" || resolved.split('/').filter(|part| !part.is_empty()).count() <= 1 {
        return true;
    }
    let below = |path: &str| path.strip_prefix(resolved).is_some_and(|rest| rest.starts_with('/'));
    if below(original) || canonical.as_deref().is_some_and(below) {
        return true;
    }
    let within = |root: &str| resolved.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'));
    !(within(original) || canonical.as_deref().is_some_and(within))
}

#[cfg(test)]
mod tests;
