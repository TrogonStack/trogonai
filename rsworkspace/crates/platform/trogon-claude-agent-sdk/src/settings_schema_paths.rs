//! Port of the helper executable path validation the policy helper schema applies (upstream `T5e`,
//! `x5e`, `dd`, `Xo`, `R5e`, `O5e` and Node's `path.win32` / `path.posix` `normalize` and
//! `isAbsolute`). Strings are walked as UTF-16 code units, as the upstream runtime does.

use std::sync::LazyLock;

use regex::bytes::{Regex, RegexBuilder};

use crate::constants::{
    SETTINGS_PATTERN_MAGIC_LINK, SETTINGS_PATTERN_NETWORK_SERVERS, SETTINGS_PATTERN_POWERSHELL_SUFFIX,
    SETTINGS_PATTERN_VOLUME_COMPONENT, SETTINGS_PATTERN_VOLUME_ROOT, SETTINGS_PATTERN_WINDOWS_EXECUTABLE,
    UTF16_BACKSLASH, UTF16_COLON, UTF16_DOT, UTF16_SLASH,
};

/// Which path rules a policy helper entry applies: `windows` entries use the Win32 rules, every other
/// platform the POSIX rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathProfile {
    Windows,
    Posix,
}

fn ascii_insensitive(pattern: &str) -> Option<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .unicode(false)
        .build()
        .ok()
}

fn matches(cell: &LazyLock<Option<Regex>>, text: &str) -> bool {
    cell.as_ref().is_some_and(|regex| regex.is_match(text.as_bytes()))
}

static VOLUME_COMPONENT: LazyLock<Option<Regex>> =
    LazyLock::new(|| ascii_insensitive(SETTINGS_PATTERN_VOLUME_COMPONENT));
static VOLUME_ROOT: LazyLock<Option<Regex>> = LazyLock::new(|| ascii_insensitive(SETTINGS_PATTERN_VOLUME_ROOT));
static NETWORK_SERVERS: LazyLock<Option<Regex>> = LazyLock::new(|| ascii_insensitive(SETTINGS_PATTERN_NETWORK_SERVERS));
static MAGIC_LINK: LazyLock<Option<Regex>> = LazyLock::new(|| ascii_insensitive(SETTINGS_PATTERN_MAGIC_LINK));
static WINDOWS_EXECUTABLE: LazyLock<Option<Regex>> =
    LazyLock::new(|| ascii_insensitive(SETTINGS_PATTERN_WINDOWS_EXECUTABLE));
static POWERSHELL_SUFFIX: LazyLock<Option<Regex>> =
    LazyLock::new(|| ascii_insensitive(SETTINGS_PATTERN_POWERSHELL_SUFFIX));

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn text_of(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn is_posix_separator(unit: u16) -> bool {
    unit == UTF16_SLASH
}

fn is_win32_separator(unit: u16) -> bool {
    unit == UTF16_SLASH || unit == UTF16_BACKSLASH
}

fn is_device_root(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|byte| byte.is_ascii_alphabetic())
}

/// Node's `normalizeString`.
fn normalize_string(path: &[u16], allow_above_root: bool, separator: u16, is_separator: fn(u16) -> bool) -> Vec<u16> {
    let mut result: Vec<u16> = Vec::new();
    let mut last_segment_length: usize = 0;
    let mut last_slash: isize = -1;
    let mut dots: isize = 0;
    let mut code: u16 = 0;
    let mut index: usize = 0;
    let last_separator = |value: &[u16]| value.iter().rposition(|unit| *unit == separator);
    while index <= path.len() {
        if index < path.len() {
            code = path[index];
        } else if is_separator(code) {
            break;
        } else {
            code = UTF16_SLASH;
        }
        if is_separator(code) {
            if last_slash == index as isize - 1 || dots == 1 {
            } else if dots == 2 {
                let ends_with_dots = result.len() >= 2
                    && last_segment_length == 2
                    && result[result.len() - 1] == UTF16_DOT
                    && result[result.len() - 2] == UTF16_DOT;
                if !ends_with_dots {
                    if result.len() > 2 {
                        match last_separator(&result) {
                            None => {
                                result.clear();
                                last_segment_length = 0;
                            }
                            Some(position) => {
                                result.truncate(position);
                                let previous = last_separator(&result).map_or(-1, |value| value as isize);
                                last_segment_length = (result.len() as isize - 1 - previous) as usize;
                            }
                        }
                        last_slash = index as isize;
                        dots = 0;
                        index += 1;
                        continue;
                    } else if !result.is_empty() {
                        result.clear();
                        last_segment_length = 0;
                        last_slash = index as isize;
                        dots = 0;
                        index += 1;
                        continue;
                    }
                }
                if allow_above_root {
                    if !result.is_empty() {
                        result.push(separator);
                    }
                    result.push(UTF16_DOT);
                    result.push(UTF16_DOT);
                    last_segment_length = 2;
                }
            } else {
                let segment = &path[(last_slash + 1) as usize..index];
                if !result.is_empty() {
                    result.push(separator);
                }
                result.extend_from_slice(segment);
                last_segment_length = (index as isize - last_slash - 1) as usize;
            }
            last_slash = index as isize;
            dots = 0;
        } else if code == UTF16_DOT && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
        index += 1;
    }
    result
}

/// Node `path.posix.normalize`.
pub(crate) fn posix_normalize(path: &str) -> String {
    let path = units(path);
    let (Some(first), Some(last)) = (path.first(), path.last()) else {
        return ".".to_owned();
    };
    let absolute = *first == UTF16_SLASH;
    let trailing = *last == UTF16_SLASH;
    let mut normalized = normalize_string(&path, !absolute, UTF16_SLASH, is_posix_separator);
    if normalized.is_empty() {
        return match (absolute, trailing) {
            (true, _) => "/".to_owned(),
            (false, true) => "./".to_owned(),
            (false, false) => ".".to_owned(),
        };
    }
    if trailing {
        normalized.push(UTF16_SLASH);
    }
    if absolute {
        normalized.insert(0, UTF16_SLASH);
    }
    text_of(&normalized)
}

fn slice_text(path: &[u16], start: usize, end: usize) -> String {
    text_of(&path[start..end])
}

/// Node `path.win32.normalize`.
pub(crate) fn win32_normalize(path: &str) -> String {
    let path = units(path);
    let length = path.len();
    if length == 0 {
        return ".".to_owned();
    }
    let first = path[0];
    if length == 1 {
        return if is_posix_separator(first) {
            "\\".to_owned()
        } else {
            text_of(&path)
        };
    }
    let mut root_end = 0;
    let mut device: Option<String> = None;
    let mut absolute = false;
    if is_win32_separator(first) {
        absolute = true;
        if is_win32_separator(path[1]) {
            let mut j = 2;
            let mut last = j;
            while j < length && !is_win32_separator(path[j]) {
                j += 1;
            }
            if j < length && j != last {
                let first_part = slice_text(&path, last, j);
                last = j;
                while j < length && is_win32_separator(path[j]) {
                    j += 1;
                }
                if j < length && j != last {
                    last = j;
                    while j < length && !is_win32_separator(path[j]) {
                        j += 1;
                    }
                    if j == length {
                        return format!("\\\\{first_part}\\{}\\", slice_text(&path, last, length));
                    }
                    if j != last {
                        device = Some(format!("\\\\{first_part}\\{}", slice_text(&path, last, j)));
                        root_end = j;
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else if is_device_root(first) && path[1] == UTF16_COLON {
        device = Some(slice_text(&path, 0, 2));
        root_end = 2;
        if length > 2 && is_win32_separator(path[2]) {
            absolute = true;
            root_end = 3;
        }
    }
    let mut tail: Vec<u16> = if root_end < length {
        normalize_string(&path[root_end..], !absolute, UTF16_BACKSLASH, is_win32_separator)
    } else {
        Vec::new()
    };
    if tail.is_empty() && !absolute {
        tail = vec![UTF16_DOT];
    }
    if !tail.is_empty() && is_win32_separator(path[length - 1]) {
        tail.push(UTF16_BACKSLASH);
    }
    let tail = text_of(&tail);
    if !absolute && device.is_none() && path.contains(&UTF16_COLON) {
        let tail_units = units(&tail);
        if tail_units.len() >= 2 && is_device_root(tail_units[0]) && tail_units[1] == UTF16_COLON {
            return format!(".\\{tail}");
        }
        let mut cursor = path.iter().position(|unit| *unit == UTF16_COLON);
        while let Some(index) = cursor {
            if index == length - 1 || is_win32_separator(path[index + 1]) {
                return format!(".\\{tail}");
            }
            cursor = path[index + 1..]
                .iter()
                .position(|unit| *unit == UTF16_COLON)
                .map(|offset| index + 1 + offset);
        }
    }
    match (device, absolute) {
        (None, true) => format!("\\{tail}"),
        (None, false) => tail,
        (Some(device), true) => format!("{device}\\{tail}"),
        (Some(device), false) => format!("{device}{tail}"),
    }
}

/// Node `path.win32.isAbsolute`.
pub(crate) fn win32_is_absolute(path: &str) -> bool {
    let path = units(path);
    match path.first() {
        None => false,
        Some(first) if is_win32_separator(*first) => true,
        Some(first) => {
            is_device_root(*first) && path.len() > 2 && path[1] == UTF16_COLON && is_win32_separator(path[2])
        }
    }
}

/// Node `path.posix.isAbsolute`.
pub(crate) fn posix_is_absolute(path: &str) -> bool {
    path.starts_with('/')
}

/// Upstream `Nb`: strips invisible controls and case-folds the way the upstream runtime does.
fn fold_name(name: &str) -> String {
    name.chars()
        .filter(|character| {
            !matches!(character, '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}')
        })
        .collect::<String>()
        .to_uppercase()
        .to_lowercase()
}

/// Upstream `zI`.
fn is_automount_root(components: &[&str]) -> bool {
    (components.len() == 2 && fold_name(components[0]) == "net")
        || (components.len() == 3 && fold_name(components[0]) == "network" && fold_name(components[1]) == "servers")
}

/// Upstream `Kr`: `dd(path) !== null`, true when the path reaches an automount root.
fn reaches_automount_root(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut components: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => {
                components.push(part);
                if is_automount_root(&components) {
                    return true;
                }
            }
        }
    }
    false
}

/// Upstream `Xo`.
fn reaches_volume_root(path: &str) -> bool {
    if !matches(&VOLUME_COMPONENT, path) || !path.starts_with('/') {
        return false;
    }
    let mut components: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => {
                components.push(part);
                if components.len() == 1 && matches(&VOLUME_ROOT, part) {
                    return true;
                }
            }
        }
    }
    false
}

fn is_separator_char(character: char) -> bool {
    character == '/' || character == '\\'
}

/// Upstream `Jo`: a device namespace prefix.
fn has_device_prefix(path: &str) -> bool {
    let mut characters = path.chars();
    matches!(
        (characters.next(), characters.next(), characters.next(), characters.next()),
        (Some(first), Some(second), Some(third), Some(fourth))
            if is_separator_char(first) && is_separator_char(second) && (third == '?' || third == '.') && is_separator_char(fourth)
    )
}

/// Upstream `A5e`.
fn is_unc(path: &str) -> bool {
    let backslashed = path.replace('/', "\\");
    if has_device_prefix(&backslashed) {
        return false;
    }
    let mut characters = backslashed.chars();
    matches!(
        (characters.next(), characters.next(), characters.next()),
        (Some('\\'), Some('\\'), Some(third)) if third != '\\'
    )
}

fn is_drive_prefix(path: &str) -> bool {
    let mut characters = path.chars();
    matches!((characters.next(), characters.next()), (Some(letter), Some(':')) if letter.is_ascii_alphabetic())
}

fn is_drive_root(path: &str) -> bool {
    is_drive_prefix(path) && path.chars().nth(2) == Some('\\')
}

fn win32_valid(path: &str) -> bool {
    let path = path.replace('/', "\\");
    if has_device_prefix(&path) {
        return false;
    }
    let unc = is_unc(&path);
    if unc {
        return false;
    }
    if !is_drive_root(&path) && !unc && (path.starts_with('\\') || is_drive_prefix(&path)) {
        return false;
    }
    if win32_normalize(&path) != path {
        return false;
    }
    let segments: Vec<&str> = path.split('\\').collect();
    if segments.iter().any(|segment| *segment == "." || *segment == "..") {
        return false;
    }
    let bad_segment = segments.iter().enumerate().any(|(position, segment)| {
        segment.ends_with(['.', ' '])
            || (segment.contains(':') && !(position == 0 && segment.len() == 2 && is_drive_prefix(segment)))
    });
    if bad_segment {
        return false;
    }
    let rest = path.strip_prefix("\\\\").unwrap_or(&path);
    if rest.contains("\\\\") {
        return false;
    }
    !path.ends_with('\\') || path == "\\" || (path.len() == 3 && is_drive_prefix(&path))
}

fn posix_valid(path: &str) -> bool {
    if reaches_automount_root(path)
        || matches(&NETWORK_SERVERS, path)
        || reaches_volume_root(path)
        || matches(&MAGIC_LINK, path)
    {
        return false;
    }
    if posix_normalize(path) != path {
        return false;
    }
    if path.split('/').any(|segment| segment == "." || segment == "..") {
        return false;
    }
    if path.contains("//") {
        return false;
    }
    !path.ends_with('/') || path == "/"
}

impl PathProfile {
    pub(crate) fn is_absolute(self, path: &str) -> bool {
        match self {
            PathProfile::Windows => win32_is_absolute(path),
            PathProfile::Posix => posix_is_absolute(path),
        }
    }

    /// Upstream `!(t==="win32"&&n.requireWin32ExecutableSuffix)||w5e.test(r)`.
    pub(crate) fn has_executable_suffix(self, path: &str) -> bool {
        self == PathProfile::Posix || matches(&WINDOWS_EXECUTABLE, path)
    }

    /// Upstream `t!=="win32"||!x5e(r)`: a PowerShell path must not contain glob characters.
    pub(crate) fn has_no_glob(self, path: &str) -> bool {
        self == PathProfile::Posix || !(matches(&POWERSHELL_SUFFIX, path) && path.contains(['[', ']', '`', '*', '?']))
    }

    /// Upstream `T5e`.
    pub(crate) fn is_normalized(self, path: &str) -> bool {
        match self {
            PathProfile::Windows => win32_valid(path),
            PathProfile::Posix => posix_valid(path),
        }
    }
}
