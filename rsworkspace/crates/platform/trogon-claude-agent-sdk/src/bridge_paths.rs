//! The path checks of the upstream bridge: which paths the repository reader refuses to touch.
//!
//! A path is refused when it reaches a network share, one of the special mount roots, an automounted host other than
//! the one the repository lives on, or (on macOS) a location the automounter may fill in. Links are followed one
//! component at a time and every target passes the same checks. Everything the checks ask of the host goes through a
//! [`PlatformProbe`], and the cache of automount roots is the one upstream keeps for the whole process.
//!
//! Path text is handled with POSIX rules, as upstream does on every platform but Windows, where the [`Flavor`] of the
//! guard follows drive letters, backslashes and UNC roots.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use crate::bridge_flavor::{Flavor, dirname, share_refused};
use crate::bridge_platform::{EntryKind, HostProbe, Platform, PlatformProbe};
use crate::bridge_unicode::js_lowercase;
use crate::session_prompt::{js_space, js_trim};
use unicode_normalization::UnicodeNormalization;

mod constants;

use constants::{ASKED_TWICE, AUTOFS_TYPE, DATA_VOLUME_ROOTS, EXPECTED_ERRORS, LINK_DEPTH, PARTIAL_DEPTH};

/// The automount roots upstream has already ruled out, and the lookups that failed once without being expected to.
#[derive(Default)]
struct AutofsCache {
    not_map_roots: HashSet<String>,
    failures: HashMap<String, u32>,
}

/// The path checks together with the host they ask and the caches they keep.
pub struct PathGuard {
    probe: Arc<dyn PlatformProbe>,
    autofs: Mutex<AutofsCache>,
    own_home_mount: Mutex<Option<bool>>,
}

impl std::fmt::Debug for PathGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PathGuard").finish_non_exhaustive()
    }
}

static HOST_GUARD: LazyLock<Arc<PathGuard>> = LazyLock::new(|| Arc::new(PathGuard::new(Arc::new(HostProbe))));

impl PathGuard {
    /// A guard that asks `probe`, with empty caches.
    pub fn new(probe: Arc<dyn PlatformProbe>) -> Self {
        Self {
            probe,
            autofs: Mutex::new(AutofsCache::default()),
            own_home_mount: Mutex::new(None),
        }
    }

    /// The guard for the real host, shared by the whole process like the upstream caches are.
    pub fn host() -> Arc<Self> {
        Arc::clone(&HOST_GUARD)
    }
}

fn fold_case(text: &str) -> String {
    js_lowercase(&text.to_uppercase())
}

fn strip_format(text: &str) -> String {
    text.chars()
        .filter(
            |c| !matches!(c, '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}'),
        )
        .collect()
}

/// The components of `path` with `.` dropped and `..` popping, reported as each one is pushed.
fn each_pushed<'a>(path: &'a str, mut visit: impl FnMut(&[&'a str], &'a str) -> bool) -> bool {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                parts.pop();
                continue;
            }
            _ => {}
        }
        parts.push(part);
        if visit(&parts, part) {
            return true;
        }
    }
    false
}

/// An absolute path that passes through a top level `network` directory.
fn network_path(path: &str) -> bool {
    path.starts_with('/') && each_pushed(path, |parts, _| parts.len() == 1 && js_lowercase(parts[0]) == "network")
}

/// An absolute path that is exactly a top level `net` directory.
fn bare_net(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.len() == 1 && js_lowercase(parts[0]) == "net"
}

/// An absolute path through one of the special top level mount roots.
fn special_mount(path: &str) -> bool {
    path.starts_with('/')
        && each_pushed(path, |parts, part| {
            parts.len() == 1
                && [".vol", ".file", ".nofollow", ".resolve"]
                    .iter()
                    .any(|name| part.eq_ignore_ascii_case(name))
        })
}

fn net_share(parts: &[&str]) -> bool {
    let named = |part: &str| fold_case(&strip_format(part));
    (parts.len() == 2 && named(parts[0]) == "net")
        || (parts.len() == 3 && named(parts[0]) == "network" && named(parts[1]) == "servers")
}

/// Strips the `/System/Volumes/Data` prefix macOS puts in front of the writable volume's own directories.
fn data_volume(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    let named = |index: usize| fold_case(parts.get(index).copied().unwrap_or_default());
    if parts.len() < 5 || !parts[0].is_empty() || named(1) != "system" || named(2) != "volumes" || named(3) != "data" {
        return path.to_owned();
    }
    let rest = &parts[4..];
    for root in DATA_VOLUME_ROOTS {
        let wanted: Vec<&str> = root.split('/').collect();
        if rest.len() < wanted.len() {
            continue;
        }
        if wanted
            .iter()
            .enumerate()
            .all(|(index, name)| fold_case(rest[index]) == fold_case(name))
        {
            return format!("/{}", rest.join("/"));
        }
    }
    path.to_owned()
}

/// What `/network` and `/home` locations a path reaches: the complete ones and the ones still being typed.
#[derive(Default)]
struct Locations {
    complete: Vec<String>,
    partial: Vec<String>,
}

fn locations(path: &str, platform: Platform, map_home: bool) -> Locations {
    let mut found = Locations::default();
    if !path.starts_with('/') {
        return found;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                parts.pop();
                continue;
            }
            _ => {}
        }
        parts.push(part);
        if parts.len() > PARTIAL_DEPTH {
            continue;
        }
        let prefix = format!("/{}", parts.join("/"));
        let shown = if platform == Platform::Darwin {
            data_volume(&prefix)
        } else {
            prefix
        };
        let pieces: Vec<&str> = shown.split('/').filter(|piece| !piece.is_empty()).collect();
        let first = pieces.first().map(|piece| js_lowercase(piece));
        let host = |index: usize| pieces[index].to_ascii_lowercase();
        if first.as_deref() == Some("network") {
            match pieces.len() {
                1 => found.partial.push("/network/".to_owned()),
                2 => found.partial.push(format!("/network/{}/", host(1))),
                3 => found.complete.push(format!("/network/{}/{}", host(1), host(2))),
                _ => {}
            }
        } else if map_home && first.as_deref() == Some("home") && pieces.len() == 2 {
            found.complete.push(format!("/home/{}", pieces[1]));
        }
    }
    found
}

/// The `/etc/auto_master` text with each line's comment removed the way the upstream pattern removes it.
fn without_comment(line: &str) -> &str {
    let ends_line = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    for (index, c) in line.char_indices() {
        if c == '#' && !line[index + 1..].chars().any(ends_line) {
            return &line[..index];
        }
    }
    line
}

fn lists_home(master: &str) -> bool {
    master.split('\n').any(|line| {
        let trimmed = js_trim(without_comment(line));
        let fields: Vec<&str> = trimmed.split(js_space).filter(|field| !field.is_empty()).collect();
        fields.len() >= 2 && js_lowercase(fields[0]) == "/home"
    })
}

impl PathGuard {
    fn platform(&self) -> Platform {
        self.probe.platform()
    }

    fn flavor(&self) -> Flavor {
        Flavor::of(self.platform())
    }

    /// The lower cased host of an automounted share path, when the path is one and the platform has them.
    pub(crate) fn automount_host(&self, path: &str) -> Option<String> {
        if self.platform() == Platform::Win32 || !path.starts_with('/') {
            return None;
        }
        let mut host = None;
        each_pushed(path, |parts, part| {
            let hit = net_share(parts);
            if hit {
                host = Some(part.to_ascii_lowercase());
            }
            hit
        });
        host
    }

    fn on_other_host(&self, path: &str, base: &str) -> bool {
        match self.automount_host(path) {
            None => false,
            Some(host) => Some(host) != self.automount_host(base),
        }
    }

    /// Whether the home directory is its own mount on macOS, which makes `/home` a place the automounter fills in.
    fn own_home_mount(&self) -> bool {
        let mut cache = self.own_home_mount.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(value) = *cache {
            return value;
        }
        let listed = match self.probe.read_text("/etc/auto_master") {
            Ok(master) => lists_home(&master),
            Err(error) if error.code.as_deref() == Some("ENOENT") => false,
            Err(_) => return true,
        };
        let value = if listed {
            true
        } else {
            match self.probe.device_of("/home") {
                Ok(home) => match self.probe.device_of("/Users") {
                    Ok(users) => home != users,
                    Err(_) => return true,
                },
                Err(error) if error.code.as_deref() == Some("ENOENT") => true,
                Err(_) => return true,
            }
        };
        *cache = Some(value);
        value
    }

    fn map_home(&self, platform: Platform) -> bool {
        platform == Platform::Darwin && (self.platform() != Platform::Darwin || self.own_home_mount())
    }

    fn home_user_directory(&self) -> Option<String> {
        let home = self.probe.home_dir()?;
        let shown = data_volume(&home);
        let name = shown.strip_prefix("/home/")?;
        let user = name.split('/').next().filter(|user| !user.is_empty())?;
        Some(format!("/home/{user}"))
    }

    fn mac_special(&self, path: &str, platform: Platform, map_home: bool) -> bool {
        if platform != Platform::Darwin {
            return false;
        }
        if network_path(path) || special_mount(path) {
            return true;
        }
        if !path.starts_with('/') {
            return false;
        }
        let found = locations(path, platform, map_home);
        if !found.partial.is_empty() || found.complete.iter().any(|entry| entry.starts_with("/network/")) {
            return true;
        }
        let homes: Vec<&String> = found
            .complete
            .iter()
            .filter(|entry| entry.starts_with("/home/"))
            .collect();
        if homes.is_empty() {
            return false;
        }
        let own = self.home_user_directory();
        homes.iter().any(|entry| Some(entry.as_str()) != own.as_deref())
    }

    /// Whether macOS may be about to mount something at `path` that is not part of `base`'s own mount tree.
    fn mac_mount_point(&self, path: &str, base: &str) -> bool {
        let platform = self.platform();
        if platform != Platform::Darwin {
            return false;
        }
        let map_home = self.map_home(platform);
        if !self.mac_special(path, platform, map_home) {
            return false;
        }
        let own = locations(base, platform, map_home).complete.into_iter().next();
        let found = locations(path, platform, map_home);
        if found.complete.is_empty() && found.partial.is_empty() {
            return true;
        }
        let Some(own) = own else {
            return true;
        };
        found.complete.iter().any(|entry| *entry != own) || found.partial.iter().any(|entry| !own.starts_with(entry))
    }

    /// Whether `path` is somewhere the walker must not step, given the directory `base` it started from.
    fn forbidden_step(&self, path: &str, base: &str) -> bool {
        special_mount(path) || self.on_other_host(path, base) || self.mac_mount_point(path, base)
    }

    /// Whether `text` (a link target or a `gitdir:` value) is refused, relative to `base`.
    pub(crate) fn refuses(&self, text: &str, base: &str) -> bool {
        let resolved = self.flavor().resolve(base, text);
        if share_refused(text, base) || share_refused(&resolved, base) {
            return true;
        }
        let own = self.automount_host(base);
        [text, resolved.as_str()].iter().any(|candidate| {
            self.automount_host(candidate).is_some_and(|host| Some(host) != own)
                || network_path(candidate)
                || bare_net(candidate)
                || special_mount(candidate)
                || self.mac_mount_point(candidate, base)
        })
    }

    /// Follows `path` one component at a time and says whether every step is allowed, `base` being where it started.
    pub(crate) fn walks(&self, path: &str, base: &str) -> bool {
        self.walk(path, base, LINK_DEPTH)
    }

    fn walk(&self, path: &str, base: &str, depth: u32) -> bool {
        if depth == 0 {
            return false;
        }
        let flavor = self.flavor();
        let (root, rest) = path.split_at(flavor.root_len(path));
        let mut current = root.to_owned();
        let parts = flavor.split(rest);
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() || *part == "." {
                continue;
            }
            if *part == ".." {
                current = flavor.dirname(&current);
                continue;
            }
            current = flavor.join(&current, part);
            if self.forbidden_step(&current, base) || network_path(&current) {
                return false;
            }
            match self.probe.entry_kind(&current) {
                EntryKind::Special | EntryKind::Other => return false,
                EntryKind::Symlink => {
                    let Some(text) = self.probe.link_text(&current) else {
                        return false;
                    };
                    let parent = flavor.dirname(&current);
                    if self.refuses(&text, &parent) {
                        return false;
                    }
                    let target = if flavor.rooted(&text) {
                        text
                    } else {
                        format!("{parent}{}{text}", flavor.sep())
                    };
                    let remaining = parts[index + 1..].join(&flavor.sep().to_string());
                    let next = if remaining.is_empty() {
                        target
                    } else if flavor.ends_with_separator(&target) {
                        format!("{target}{remaining}")
                    } else {
                        format!("{target}{}{remaining}", flavor.sep())
                    };
                    return self.walk(&next, base, depth - 1);
                }
                EntryKind::Absent | EntryKind::File | EntryKind::Directory => {}
            }
        }
        true
    }

    /// Where `text` leads from `base`, when neither the text nor any step of the way is refused.
    pub(crate) fn reach(&self, base: &str, text: &str) -> Option<String> {
        if self.refuses(text, base) {
            return None;
        }
        let flavor = self.flavor();
        let joined = if flavor.rooted(text) {
            text.to_owned()
        } else {
            format!("{base}{}{text}", flavor.sep())
        };
        let resolved = flavor.resolve(base, text);
        (self.walks(&joined, base) && (resolved == joined || self.walks(&resolved, base))).then_some(resolved)
    }

    /// `path.join(base, name)` for the platform the guard asks.
    pub(crate) fn join(&self, base: &str, name: &str) -> String {
        self.flavor().join(base, name)
    }

    /// The directory of the nearest repository around `directory`, skipping automount map roots.
    pub(crate) fn find_root(&self, directory: &str) -> Option<String> {
        let flavor = self.flavor();
        let mut current = flavor.resolve_alone(directory);
        let root = current
            .find(flavor.sep())
            .map_or_else(|| flavor.sep().to_string(), |index| current[..=index].to_owned());
        while current != root {
            if !self.autofs_root(&current) && self.has_repository_marker(&current) {
                return Some(self.precomposed(&current));
            }
            let parent = flavor.dirname(&current);
            if parent == current {
                break;
            }
            current = parent;
        }
        self.has_repository_marker(&root).then(|| self.precomposed(&root))
    }

    /// The path in the form macOS file names are compared in.
    fn precomposed(&self, path: &str) -> String {
        if self.platform() == Platform::Darwin {
            path.nfc().collect()
        } else {
            path.to_owned()
        }
    }

    /// What `path` is, without following a final link.
    pub(crate) fn entry_kind(&self, path: &str) -> EntryKind {
        self.probe.entry_kind(path)
    }

    /// What `path` is once links are followed.
    pub(crate) fn followed_kind(&self, path: &str) -> EntryKind {
        self.probe.followed_kind(path)
    }

    /// Whether `directory` holds a usable `.git` entry: a directory or file, or a link to one that passes the checks.
    pub(crate) fn has_repository_marker(&self, directory: &str) -> bool {
        let marker = self.join(directory, ".git");
        match self.probe.entry_kind(&marker) {
            EntryKind::Symlink => self.probe.link_text(&marker).is_some_and(|target| {
                self.reach(directory, &target).is_some()
                    && matches!(
                        self.probe.followed_kind(&marker),
                        EntryKind::File | EntryKind::Directory
                    )
            }),
            EntryKind::File | EntryKind::Directory => true,
            _ => false,
        }
    }

    /// Whether `path`, a top level directory, is an automount map root the upstream finder skips.
    pub(crate) fn autofs_root(&self, path: &str) -> bool {
        if !matches!(self.probe.runtime_platform(), Platform::Linux | Platform::Wsl)
            || path == "/"
            || dirname(path) != "/"
        {
            return false;
        }
        let mut cache = self.autofs.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.not_map_roots.contains(path) {
            return false;
        }
        match self.probe.statfs_type(path) {
            Ok(AUTOFS_TYPE) => {
                cache.failures.remove(path);
                return true;
            }
            Ok(_) => {}
            Err(error) => {
                let expected = match &error.code {
                    None => error.type_error,
                    Some(code) => EXPECTED_ERRORS.contains(&code.as_str()),
                };
                let failures = cache.failures.get(path).copied().unwrap_or_default() + 1;
                if !expected && failures < ASKED_TWICE {
                    cache.failures.insert(path.to_owned(), failures);
                    return false;
                }
            }
        }
        cache.not_map_roots.insert(path.to_owned());
        false
    }
}

#[cfg(test)]
#[path = "bridge_paths_tests.rs"]
mod tests;
