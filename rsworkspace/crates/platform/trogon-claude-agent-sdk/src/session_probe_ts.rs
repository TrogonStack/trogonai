//! TypeScript SDK 0.3.323 path safety rules used before the SDK touches a directory recorded in a transcript.
//!
//! A transcript's `cwd` is data, so the SDK refuses to resolve it when that could reach a network share or a device
//! namespace. `Gr` is the purely lexical test, `iM` adds a symlink walk (`Ed`/`aM`) that follows the path one
//! component at a time without ever resolving a link target on the machine's behalf. Both are ported rule for rule,
//! with the options `l0` leaves unset (no anchors, no unreadable-ancestry mode).
//!
//! Not covered: the Windows walk (`Ne.resolve` and drive roots), and the `openDirNoFollow` retry after an unexpected
//! `lstat` failure, which can only succeed where `lstat` itself succeeded, so any `lstat` failure ends the walk.

#[cfg(unix)]
use std::collections::{HashSet, VecDeque};

#[cfg(unix)]
use crate::constants::SYMLINK_HOP_LIMIT;

fn is_slash(ch: char) -> bool {
    matches!(ch, '/' | '\\')
}

fn starts_with_two_slashes(path: &str) -> bool {
    let mut chars = path.chars();
    chars.next().is_some_and(is_slash) && chars.next().is_some_and(is_slash)
}

/// Upstream `vl`: some component is `..`.
pub(crate) fn has_parent_segment(path: &str) -> bool {
    path.split(is_slash).any(|segment| segment == "..")
}

/// Upstream `/^[\\/]\?\?[\\/]/` (`MI`).
fn starts_with_nt_namespace(path: &str) -> bool {
    let mut chars = path.chars();
    chars.next().is_some_and(is_slash)
        && chars.next() == Some('?')
        && chars.next() == Some('?')
        && chars.next().is_some_and(is_slash)
}

/// `path.win32.normalize(path)` for a path with a single leading separator, which is the only shape whose result
/// can start with `\??\` without already starting with two separators.
fn win32_normalized_nt_namespace(path: &str) -> bool {
    let mut chars = path.chars();
    if !chars.next().is_some_and(is_slash) || chars.next().is_some_and(is_slash) {
        return false;
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split(is_slash) {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let trailing = path.ends_with(is_slash) && !segments.is_empty();
    segments.first() == Some(&"??") && (segments.len() > 1 || trailing)
}

/// Upstream `Wm`.
fn nt_namespace(path: &str) -> bool {
    starts_with_nt_namespace(path) || (path.contains("??") && win32_normalized_nt_namespace(path))
}

/// Upstream `Pn`.
fn unc_like(path: &str) -> bool {
    starts_with_two_slashes(path) || nt_namespace(path)
}

/// Upstream `Or`: a WSL share with a real distribution name.
fn wsl_share(path: &str) -> bool {
    let mut chars = path.chars();
    if !(chars.next().is_some_and(is_slash) && chars.next().is_some_and(is_slash)) {
        return false;
    }
    let rest = &path[2..];
    let lowered = rest.to_ascii_lowercase();
    let after = if lowered.starts_with("wsl$") {
        4
    } else if lowered.starts_with("wsl.localhost") {
        13
    } else {
        return false;
    };
    let mut tail = rest[after..].chars();
    if !tail.next().is_some_and(is_slash) {
        return false;
    }
    tail.take_while(|ch| !is_slash(*ch)).any(|ch| ch != '.' && ch != ' ')
}

/// Upstream `xl`: a UNC or namespace path that is not a WSL share.
fn foreign_share(path: &str) -> bool {
    unc_like(path) && !wsl_share(path)
}

/// Upstream `Jo`: `\\?\` and `\\.\`.
fn device_prefix(path: &str) -> bool {
    let mut chars = path.chars();
    chars.next().is_some_and(is_slash)
        && chars.next().is_some_and(is_slash)
        && matches!(chars.next(), Some('?' | '.'))
        && chars.next().is_some_and(is_slash)
}

/// Upstream `HI`.
fn nt_object_root(path: &str) -> bool {
    let mut chars = path.chars();
    if !chars.next().is_some_and(is_slash) {
        return false;
    }
    let rest = path[1..].to_ascii_lowercase();
    ["global??", "globalroot", "dosdevices", "device"].iter().any(|name| {
        rest.strip_prefix(name)
            .is_some_and(|after| after.chars().next().is_some_and(is_slash))
    })
}

/// The `/a/./b/../c` walk shared by `vI`, `wI`, `Xo` and `Qo`: segments with `.` and empty parts dropped and `..`
/// popping, `stop` deciding after each push.
fn walk_segments(path: &str, mut stop: impl FnMut(&[&str]) -> bool) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => {
                segments.push(other);
                if stop(&segments) {
                    return true;
                }
            }
        }
    }
    false
}

/// Upstream `Nb`: zero-width and bidi controls removed, then case-folded through upper case.
fn fold_segment(segment: &str) -> String {
    segment
        .chars()
        .filter(|ch| {
            !matches!(
                *ch,
                '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}'
            )
        })
        .collect::<String>()
        .to_uppercase()
        .to_lowercase()
}

/// Upstream `Kr`: `/net/<host>` and `/network/servers/<host>`, the macOS and Linux automounted network trees.
fn network_mount(path: &str) -> bool {
    walk_segments(path, |segments| {
        (segments.len() == 2 && fold_segment(segments[0]) == "net")
            || (segments.len() == 3 && fold_segment(segments[0]) == "network" && fold_segment(segments[1]) == "servers")
    })
}

/// Upstream `go`: `/network` as the first component.
fn network_root(path: &str) -> bool {
    walk_segments(path, |segments| {
        segments.len() == 1 && segments[0].to_lowercase() == "network"
    })
}

/// Upstream `Xo`: `/.vol`, `/.file`, `/.nofollow` and `/.resolve`, the macOS file-id namespaces.
fn file_id_namespace(path: &str) -> bool {
    walk_segments(path, |segments| {
        segments.len() == 1
            && [".vol", ".file", ".nofollow", ".resolve"]
                .iter()
                .any(|name| segments[0].eq_ignore_ascii_case(name))
    })
}

/// The normalized segments of an absolute path, `None` for a relative one.
fn normalized_segments(path: &str) -> Option<Vec<&str>> {
    if !path.starts_with('/') {
        return None;
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    Some(segments)
}

/// Upstream `Qo`: exactly `/net`.
fn bare_net(path: &str) -> bool {
    normalized_segments(path).is_some_and(|segments| segments.len() == 1 && segments[0].to_lowercase() == "net")
}

/// Upstream `Gr`: the path names a network share, device namespace or automounted network tree.
pub(crate) fn is_network_or_device_path(path: &str) -> bool {
    foreign_share(path)
        || network_mount(path)
        || bare_net(path)
        || device_prefix(path)
        || nt_namespace(path)
        || nt_object_root(path)
        || file_id_namespace(path)
        || network_root(path)
}

/// The synchronous part of upstream `iM`.
pub(crate) fn blocks_probe_lexically(path: &str) -> bool {
    has_parent_segment(path) || foreign_share(path) || network_mount(path) || bare_net(path)
}

/// Upstream `iM`: the path must not be probed, lexically or because walking it crosses a link into one of those
/// trees.
pub(crate) fn blocks_probe(path: &str) -> bool {
    blocks_probe_lexically(path) || walk_reaches_blocked(path)
}

fn join_segment(base: &str, segment: &str) -> String {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    match segment {
        "." => {}
        ".." => {
            parts.pop();
        }
        other => parts.push(other),
    }
    format!("/{}", parts.join("/"))
}

#[cfg(unix)]
fn under_dev_fd(path: &str) -> bool {
    path == "/dev/fd" || path.starts_with("/dev/fd/")
}

/// Upstream `Ed(UC, path)` with no options: `Some` for every outcome that is not "resolved".
#[cfg(unix)]
fn walk_reaches_blocked(path: &str) -> bool {
    if network_root(path) {
        return true;
    }
    if foreign_share(path) || network_mount(path) {
        return false;
    }
    let Ok(absolute) = std::path::absolute(if path.is_empty() { "." } else { path }) else {
        return false;
    };
    let absolute = absolute.to_string_lossy().into_owned();
    let mut segments: VecDeque<String> = VecDeque::new();
    for segment in absolute.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop_back();
            }
            other => segments.push_back(other.to_owned()),
        }
    }
    let mut current = String::from("/");
    let mut hops = 0_usize;
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(first) = segments.front() {
        if hops >= SYMLINK_HOP_LIMIT {
            break;
        }
        let candidate = join_segment(&current, first);
        if network_mount(&candidate) || network_root(&candidate) || file_id_namespace(&candidate) {
            return true;
        }
        let in_dev_fd = cfg!(target_os = "macos") && candidate != "/dev/fd" && under_dev_fd(&candidate);
        if in_dev_fd
            && candidate.rsplit_once('/').map(|(head, _)| head) == Some("/dev/fd")
            && segments.iter().skip(1).any(|part| part == ".." || part == ".")
        {
            segments.pop_front();
            while segments.len() > 1 && segments.front().is_some_and(|part| part == ".") {
                segments.pop_front();
            }
            current = candidate;
            continue;
        }
        if in_dev_fd {
            return false;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            return false;
        };
        if !metadata.file_type().is_symlink() {
            segments.pop_front();
            current = candidate;
            continue;
        }
        hops += 1;
        let key = format!(
            "{candidate}\0{}",
            segments.iter().map(String::as_str).collect::<Vec<_>>().join("\0")
        );
        if !seen.insert(key) {
            return false;
        }
        let Ok(link) = std::fs::read_link(&candidate) else {
            return false;
        };
        let link = link.to_string_lossy().into_owned();
        segments.pop_front();
        if !link.starts_with('/') {
            for part in link.split('/').filter(|part| !part.is_empty()).rev() {
                segments.push_front(part.to_owned());
            }
            continue;
        }
        if foreign_share(&link) || network_mount(&link) || network_root(&link) {
            return true;
        }
        for part in link.split('/').filter(|part| !part.is_empty()).rev() {
            segments.push_front(part.to_owned());
        }
        current = String::from("/");
    }
    if !segments.is_empty() {
        return true;
    }
    (hops > 0 && bare_net(&current)) || network_root(&current)
}

#[cfg(not(unix))]
fn walk_reaches_blocked(_path: &str) -> bool {
    false
}

#[cfg(test)]
#[path = "session_probe_ts_tests.rs"]
mod tests;
