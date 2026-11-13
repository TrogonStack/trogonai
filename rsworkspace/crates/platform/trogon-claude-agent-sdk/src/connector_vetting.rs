//! Ports the pinned bundle's path vetting walk (`bd` and the helpers it calls).
//!
//! The connector folder check vets a `.gitignore` path before it trusts what the folder holds: it walks the path one
//! component at a time, follows symbolic links itself, and refuses a path whose links lead onto a network share or
//! automount, or that it could not examine. Every helper keeps the upstream name in its doc comment and works on
//! UTF-16 code units the way the JS strings do, and the host is read at the same points and in the same order.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};

use unicode_normalization::UnicodeNormalization;

use crate::constants::{
    LINK_HOP_LIMIT, NODE_PATH_CHAR_BACKWARD_SLASH as BACKSLASH, NODE_PATH_CHAR_DOT as DOT,
    NODE_PATH_CHAR_FORWARD_SLASH as SLASH, VETTING_AUTOMOUNT_DEPTH, VETTING_BACKGROUND_SESSION_KIND, VETTING_DEV_FD,
    VETTING_FIRMLINKED_FOLDERS, VETTING_HOME_MOUNT, VETTING_REFUSAL_CODE, VETTING_SESSION_KIND_VARIABLE,
    VETTING_USERS_MOUNT,
};
use crate::node_path::{JsString, NodeError, NodePath, NodePathDialect, NodeProcess, js, lower, map_runs, upper};
use crate::session_prompt::{js_space, js_trim};

#[cfg(test)]
#[path = "connector_vetting_tests.rs"]
pub(crate) mod tests;

/// `process.platform`, as far as the walk branches on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Darwin,
    Win32,
    Other,
}

/// The `.code` of what a host read threw (`None` when it carried no string code).
pub(crate) type Errno = Option<String>;

/// What `lstat` answers about one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LinkStat {
    pub(crate) symlink: bool,
    pub(crate) file: bool,
    pub(crate) size: u64,
}

/// The host reads the walk and the connector check make (upstream `UC`, `TI`, `os.homedir` and `node:fs`).
pub(crate) trait VettingHost: NodeProcess {
    /// `lstatSync`.
    fn lstat(&self, path: &[u16]) -> Result<LinkStat, Errno>;
    /// `openDirNoFollowSync`: opens the folder without following a final link, then closes it.
    fn open_dir_no_follow(&self, path: &[u16]) -> Result<(), Errno>;
    /// `readlinkSync`.
    fn read_link(&self, path: &[u16]) -> Result<JsString, Errno>;
    /// The entry names `readdirSync` lists.
    fn read_dir_names(&self, path: &[u16]) -> Result<Vec<JsString>, Errno>;
    /// Opens the file without following a final link and reads at most `limit` bytes from its start.
    fn read_head(&self, path: &[u16], limit: usize) -> Result<Vec<u8>, Errno>;
    /// `readFileSync("/etc/auto_master", "utf8")`.
    fn read_auto_master(&self) -> Result<String, Errno>;
    /// `statSync(path).dev`.
    fn stat_device(&self, path: &str) -> Result<u64, Errno>;
    /// `os.homedir()`.
    fn home_directory(&self) -> Result<JsString, Errno>;
}

/// The `reason` a refused path carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefusalReason {
    /// A component could not be examined, or a link chain was too long or unreadable.
    Unexamined,
    /// A link leads onto a network share or automount path.
    Network,
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the settings walk only asks whether the path was refused; the probe replays read why"
    )
)]
impl RefusalReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unexamined => "unexamined",
            Self::Network => "network",
        }
    }
}

/// What `bd` throws.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the settings walk only asks whether the path was refused; the probe replays read why"
    )
)]
pub(crate) enum VetError {
    /// The walk refused the path (code `ERR_SYMLINK_TO_NETWORK_PATH`).
    Refused(RefusalReason),
    /// A path call threw, such as reading a working directory that is gone.
    Thrown(NodeError),
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the settings walk only asks whether the path was refused; the probe replays read why"
    )
)]
impl VetError {
    pub(crate) fn code(&self) -> Option<&str> {
        match self {
            Self::Refused(_) => Some(VETTING_REFUSAL_CODE),
            Self::Thrown(error) => error.code.as_deref(),
        }
    }

    pub(crate) fn reason(&self) -> Option<RefusalReason> {
        match self {
            Self::Refused(reason) => Some(*reason),
            Self::Thrown(_) => None,
        }
    }
}

impl From<NodeError> for VetError {
    fn from(error: NodeError) -> Self {
        Self::Thrown(error)
    }
}

/// What `aM` returns: `undefined`, a landing path, `Wr` or `Sd`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Walked {
    Done,
    Path(JsString),
    Unverified,
    Opaque,
}

/// The first component the walk found missing (`absentAt` upstream).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the settings walk only asks whether a component is missing; the probe replays read which one"
    )
)]
pub(crate) struct Absent {
    pub(crate) at: JsString,
    pub(crate) remaining: Vec<JsString>,
    pub(crate) from_link_target: bool,
    pub(crate) not_a_folder: bool,
}

/// What the walk reports through `onOutcome`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Absent(Absent),
    Resolved(JsString),
}

/// What `bd` returns for a path it accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Vetted {
    pub(crate) absent_at: Option<Absent>,
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the settings walk does not use where the links led; the probe replays read it"
        )
    )]
    pub(crate) links_resolved_path: Option<JsString>,
}

/// The options `Qee` hands the walk.
#[derive(Clone, Debug, Default)]
struct WalkOptions {
    anchors: Option<Vec<JsString>>,
    unverified: bool,
    launch_ancestry: Option<JsString>,
    opaque_link_text: bool,
}

/// State the bundle keeps for the life of the process (`Ub`, whether `/home` is automounted).
#[derive(Debug, Default)]
pub(crate) struct VettingState {
    home_automount: Mutex<Option<bool>>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The walk for one host, platform and node release.
pub(crate) struct Vetter<'a> {
    host: &'a dyn VettingHost,
    path: NodePath<'a>,
    platform: Platform,
    /// The working directory when the bundle loaded (`XE`), if it could be read.
    launch: Option<&'a [u16]>,
    state: &'a VettingState,
}

fn is(text: &[u16], expected: &str) -> bool {
    text.iter().copied().eq(expected.encode_utf16())
}

fn starts_with(text: &[u16], prefix: &str) -> bool {
    let prefix = js(prefix);
    text.starts_with(&prefix)
}

pub(crate) fn is_slash(unit: u16) -> bool {
    unit == SLASH || unit == BACKSLASH
}

fn unit_is(text: &[u16], index: usize, expected: char) -> bool {
    text.get(index)
        .is_some_and(|unit| u32::from(*unit) == u32::from(expected))
}

fn slash_at(text: &[u16], index: usize) -> bool {
    text.get(index).is_some_and(|unit| is_slash(*unit))
}

fn concat(parts: &[&[u16]]) -> JsString {
    parts.concat()
}

/// Compares with `expected` ignoring ASCII case only, as a non-unicode `/i` regular expression does.
fn eq_ascii_ci(text: &[u16], expected: &str) -> bool {
    text.len() == expected.len()
        && text
            .iter()
            .zip(expected.bytes())
            .all(|(unit, byte)| u8::try_from(*unit).is_ok_and(|unit| unit.eq_ignore_ascii_case(&byte)))
}

/// The leading run of units that are not `/` or `\`.
fn non_slash_run(text: &[u16]) -> &[u16] {
    let end = text.iter().position(|unit| is_slash(*unit)).unwrap_or(text.len());
    text.get(..end).unwrap_or_default()
}

/// `jm`: lowercases ASCII `A` to `Z` only.
fn ascii_lower(text: &[u16]) -> JsString {
    text.iter()
        .map(|unit| match u8::try_from(*unit) {
            Ok(byte) if byte.is_ascii_uppercase() => u16::from(byte.to_ascii_lowercase()),
            _ => *unit,
        })
        .collect()
}

/// `cd`: `toUpperCase().toLowerCase()`.
fn fold(text: &[u16]) -> JsString {
    lower(&upper(text))
}

pub(crate) fn is_invisible(unit: u16) -> bool {
    matches!(unit, 0x200c..=0x200f | 0x202a..=0x202e | 0x206a..=0x206f | 0xfeff)
}

/// `Nb`: drops the invisible format characters, then folds case.
fn plain_name(text: &[u16]) -> JsString {
    let visible: JsString = text.iter().copied().filter(|unit| !is_invisible(*unit)).collect();
    fold(&visible)
}

/// Splits on `/` the way the climbing helpers do, keeping a stack of the components still standing.
fn climb<'s, T>(path: &'s [u16], mut check: impl FnMut(&[&'s [u16]]) -> Option<T>) -> Option<T> {
    let mut stack: Vec<&'s [u16]> = Vec::new();
    for part in path.split(|unit| *unit == SLASH) {
        if part.is_empty() || is(part, ".") {
            continue;
        }
        if is(part, "..") {
            stack.pop();
            continue;
        }
        stack.push(part);
        if let Some(found) = check(&stack) {
            return Some(found);
        }
    }
    None
}

fn starts_with_slash(path: &[u16]) -> bool {
    path.first() == Some(&SLASH)
}

/// `vI` behind `go` (and `ha`): the path climbs into `/network`.
fn on_network_mount(path: &[u16]) -> bool {
    starts_with_slash(path)
        && climb(path, |stack| {
            (stack.len() == 1 && stack.first().is_some_and(|first| is(&lower(first), "network"))).then_some(())
        })
        .is_some()
}

/// `zI`: the stack names a share under `/net` or `/Network/Servers`.
fn names_share(stack: &[&[u16]]) -> bool {
    match stack {
        [first, _] => is(&plain_name(first), "net"),
        [first, second, _] => is(&plain_name(first), "network") && is(&plain_name(second), "servers"),
        _ => false,
    }
}

/// `dd` (`wI`): the automount share the path climbs into, as `/` and the components up to it.
fn share_mount(path: &[u16]) -> Option<JsString> {
    if !starts_with_slash(path) {
        return None;
    }
    climb(path, |stack| {
        names_share(stack).then(|| {
            let mut out = vec![SLASH];
            out.extend(stack.join(&SLASH));
            out
        })
    })
}

fn is_dot_name(part: &[u16]) -> bool {
    part.first() == Some(&DOT)
        && ["vol", "file", "nofollow", "resolve"]
            .iter()
            .any(|name| part.get(1..).is_some_and(|rest| eq_ascii_ci(rest, name)))
}

/// `Xo`: the path climbs into one of macOS's `/.vol`, `/.file`, `/.nofollow` or `/.resolve` namespaces.
fn in_dot_namespace(path: &[u16]) -> bool {
    starts_with_slash(path) && climb(path, |stack| matches!(stack, [only] if is_dot_name(only)).then_some(())).is_some()
}

/// The components still standing once every `..` of a `/` path is applied.
fn final_stack(path: &[u16]) -> Vec<&[u16]> {
    let mut stack = Vec::new();
    for part in path.split(|unit| *unit == SLASH) {
        if part.is_empty() || is(part, ".") {
            continue;
        }
        if is(part, "..") {
            stack.pop();
            continue;
        }
        stack.push(part);
    }
    stack
}

/// `Qo`: the path climbs to `/net` itself.
fn is_net_root(path: &[u16]) -> bool {
    starts_with_slash(path) && matches!(final_stack(path).as_slice(), [only] if is(&lower(only), "net"))
}

/// `es`: drops the `\\?\` prefix from a drive or UNC spelling.
fn strip_device_prefix(path: &[u16]) -> JsString {
    if starts_with(path, "\\\\?\\UNC\\") {
        return concat(&[&js("\\\\"), path.get(8..).unwrap_or_default()]);
    }
    if starts_with(path, "\\\\?\\") && path.len() >= 7 && unit_is(path, 5, ':') {
        return path.get(4..).unwrap_or_default().to_vec();
    }
    path.to_vec()
}

/// `Jo`: `\\?\` or `\\.\` with either separator.
fn is_device_namespace(path: &[u16]) -> bool {
    slash_at(path, 0) && slash_at(path, 1) && (unit_is(path, 2, '?') || unit_is(path, 2, '.')) && slash_at(path, 3)
}

/// `Gb` (and `MI`): `\??\` with either separator.
fn is_object_namespace(path: &[u16]) -> bool {
    slash_at(path, 0) && unit_is(path, 1, '?') && unit_is(path, 2, '?') && slash_at(path, 3)
}

/// `OI`: the lowercased server of a UNC path.
fn share_server(path: &[u16]) -> Option<JsString> {
    if !(slash_at(path, 0) && slash_at(path, 1)) {
        return None;
    }
    let device = is_device_namespace(path);
    let unc = device && path.get(4..7).is_some_and(|name| eq_ascii_ci(name, "unc")) && slash_at(path, 7);
    if device && !unc {
        return None;
    }
    if unc {
        let server = non_slash_run(path.get(8..).unwrap_or_default());
        if !server.is_empty() {
            return Some(ascii_lower(server));
        }
    }
    let server = non_slash_run(path.get(2..).unwrap_or_default());
    (!server.is_empty()).then(|| ascii_lower(server))
}

/// `Or`: a `\\wsl$\<distro>` or `\\wsl.localhost\<distro>` path with a real distribution name.
fn is_wsl_share(path: &[u16]) -> bool {
    if !(slash_at(path, 0) && slash_at(path, 1)) {
        return false;
    }
    // Both leading separators were just checked, so there is always a rest, possibly empty.
    let rest = path.get(2..).unwrap_or_default();
    if !rest.get(..3).is_some_and(|name| eq_ascii_ci(name, "wsl")) {
        return false;
    }
    let after = rest.get(3..).unwrap_or_default();
    let after = if unit_is(after, 0, '$') {
        after.get(1..).unwrap_or_default()
    } else if after.get(..10).is_some_and(|name| eq_ascii_ci(name, ".localhost")) {
        after.get(10..).unwrap_or_default()
    } else {
        return false;
    };
    if !slash_at(after, 0) {
        return false;
    }
    non_slash_run(after.get(1..).unwrap_or_default())
        .iter()
        .any(|unit| *unit != DOT && *unit != u16::from(b' '))
}

/// `Yn`: a component that starts with `.` and holds only dots and spaces.
fn has_dotted_component(path: &[u16]) -> bool {
    path.split(|unit| is_slash(*unit))
        .any(|part| part.first() == Some(&DOT) && part.iter().all(|unit| *unit == DOT || *unit == u16::from(b' ')))
}

/// `vl`: a `..` component.
fn has_parent_component(path: &[u16]) -> bool {
    path.split(|unit| is_slash(*unit)).any(|part| is(part, ".."))
}

fn trim_trailing_slashes(path: &[u16]) -> &[u16] {
    if path.len() <= 1 {
        return path;
    }
    let end = path
        .iter()
        .rposition(|unit| *unit != SLASH)
        .map_or(0, |index| index + 1);
    path.get(..end).unwrap_or_default()
}

/// `MC`: `path` is `folder` or inside it, comparing `/` spellings.
fn is_within(path: &[u16], folder: &[u16]) -> bool {
    let path = trim_trailing_slashes(path);
    let folder = trim_trailing_slashes(folder);
    if path == folder {
        return true;
    }
    if is(folder, "/") {
        return path.starts_with(folder);
    }
    let mut prefix = folder.to_vec();
    prefix.push(SLASH);
    path.starts_with(&prefix)
}

/// `XI`: maps a `/System/Volumes/Data/<firmlinked folder>` path onto the folder at the root.
fn firmlink_path(path: &[u16]) -> JsString {
    let parts: Vec<&[u16]> = path.split(|unit| *unit == SLASH).collect();
    let data_volume = match parts.as_slice() {
        [first, system, volumes, data, _, ..] => {
            first.is_empty() && is(&fold(system), "system") && is(&fold(volumes), "volumes") && is(&fold(data), "data")
        }
        _ => false,
    };
    if !data_volume {
        return path.to_vec();
    }
    let rest = parts.get(4..).unwrap_or_default();
    for folder in VETTING_FIRMLINKED_FOLDERS {
        let names: Vec<&str> = folder.split('/').collect();
        if rest.len() < names.len() {
            continue;
        }
        if names.iter().zip(rest).all(|(name, part)| fold(part) == fold(&js(name))) {
            let mut out = vec![SLASH];
            out.extend(rest.join(&SLASH));
            return out;
        }
    }
    path.to_vec()
}

/// The `/\s+/` pieces of one `/etc/auto_master` line after its comment.
fn maps_home(line: &str) -> bool {
    let start = line
        .char_indices()
        .rfind(|(_, ch)| matches!(ch, '\r' | '\u{2028}' | '\u{2029}'))
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let cut = line
        .get(start..)
        .and_then(|tail| tail.find('#'))
        .map_or(line.len(), |index| start + index);
    let kept = line.get(..cut).unwrap_or(line);
    let pieces: Vec<&str> = js_trim(kept)
        .split(js_space)
        .filter(|piece| !piece.is_empty())
        .collect();
    pieces.len() >= 2 && pieces.first().is_some_and(|first| first.to_lowercase() == "/home")
}

/// `String(value).trim()` on code units.
fn trim_units(text: &[u16]) -> &[u16] {
    let space = |unit: &u16| char::from_u32(u32::from(*unit)).is_some_and(js_space);
    let start = text.iter().position(|unit| !space(unit)).unwrap_or(text.len());
    let end = text
        .iter()
        .rposition(|unit| !space(unit))
        .map_or(start, |index| index + 1);
    text.get(start..end).unwrap_or_default()
}

/// The `IC` pieces of a path: maximal runs of separators and of everything else.
fn runs(path: &[u16]) -> Vec<&[u16]> {
    let mut pieces = Vec::new();
    let mut start = 0;
    for index in 1..=path.len() {
        let boundary = index == path.len()
            || path
                .get(index)
                .zip(path.get(index - 1))
                .is_some_and(|(unit, previous)| is_slash(*unit) != is_slash(*previous));
        if boundary {
            pieces.push(path.get(start..index).unwrap_or_default());
            start = index;
        }
    }
    pieces
}

/// `replace(/(?<=[^\\/])[\\/]+$/, "")`.
fn trim_trailing_separators(path: &[u16]) -> &[u16] {
    let end = path
        .iter()
        .rposition(|unit| !is_slash(*unit))
        .map_or(0, |index| index + 1);
    if end == 0 {
        return path;
    }
    path.get(..end).unwrap_or(path)
}

/// `IC`: keeps the path up to its first `.` or `..` component.
fn collapse_at_dots(path: &[u16]) -> JsString {
    let mut kept = JsString::new();
    for piece in runs(path) {
        if is(piece, ".") || is(piece, "..") {
            break;
        }
        kept.extend_from_slice(piece);
    }
    let trimmed = trim_trailing_separators(&kept).to_vec();
    if trimmed.is_empty() { path.to_vec() } else { trimmed }
}

/// `jee`: the POSIX collapse of a path whose components are dots.
fn collapse_posix(path: &[u16]) -> JsString {
    if starts_with(path, "\\\\") {
        return collapse_at_dots(path);
    }
    let leading = path.iter().take_while(|unit| **unit == SLASH).count();
    let single = if leading > 0 {
        concat(&[&[SLASH], path.get(leading..).unwrap_or_default()])
    } else {
        path.to_vec()
    };
    share_mount(&single).unwrap_or_else(|| collapse_at_dots(path))
}

/// `replace(/^(?:\.\.(?:[\\/]+|$))+/, "")`.
fn strip_leading_parent_steps(path: &[u16]) -> &[u16] {
    let mut rest = path;
    while let Some(after) = rest.strip_prefix(&[DOT, DOT][..]) {
        if after.is_empty() {
            return after;
        }
        let separators = after.iter().take_while(|unit| is_slash(**unit)).count();
        if separators == 0 {
            break;
        }
        rest = after.get(separators..).unwrap_or_default();
    }
    rest
}

fn ends_with_dot_or_space(part: &[u16]) -> bool {
    part.last().is_some_and(|unit| *unit == DOT || *unit == u16::from(b' '))
}

/// `nM`: whether a failed read leaves the path unverified.
fn read_failure(errno: Option<&str>, unverified: bool, hopped: bool, dotted: bool, trailing: bool) -> Walked {
    if !unverified || matches!(errno, Some("ENOENT" | "ENOTDIR")) {
        return Walked::Done;
    }
    if errno == Some("ENAMETOOLONG") && !hopped && !dotted && !trailing {
        return Walked::Done;
    }
    Walked::Unverified
}

impl<'a> Vetter<'a> {
    pub(crate) fn new(
        host: &'a dyn VettingHost,
        platform: Platform,
        dialect: NodePathDialect,
        launch: Option<&'a [u16]>,
        state: &'a VettingState,
    ) -> Self {
        Self {
            host,
            path: NodePath {
                dialect,
                windows_host: platform == Platform::Win32,
                process: host,
            },
            platform,
            launch,
            state,
        }
    }

    pub(crate) fn host(&self) -> &'a dyn VettingHost {
        self.host
    }

    fn win32(&self) -> bool {
        self.platform == Platform::Win32
    }

    fn darwin(&self) -> bool {
        self.platform == Platform::Darwin
    }

    /// `Ne.sep`.
    fn sep(&self) -> u16 {
        if self.win32() { BACKSLASH } else { SLASH }
    }

    pub(crate) fn resolve(&self, args: &[&[u16]]) -> Result<JsString, NodeError> {
        if self.win32() {
            self.path.win32_resolve(args)
        } else {
            self.path.posix_resolve(args)
        }
    }

    fn join(&self, args: &[&[u16]]) -> JsString {
        if self.win32() {
            self.path.win32_join(args)
        } else {
            self.path.posix_join(args)
        }
    }

    fn root(&self, path: &[u16]) -> JsString {
        if self.win32() {
            self.path.win32_parse(path).root
        } else {
            self.path.posix_parse(path).root
        }
    }

    fn is_absolute(&self, path: &[u16]) -> bool {
        if self.win32() {
            self.path.win32_is_absolute(path)
        } else {
            self.path.posix_is_absolute(path)
        }
    }

    /// `split(Dl).filter(Boolean)`.
    fn components(&self, path: &[u16]) -> Vec<JsString> {
        let win32 = self.win32();
        path.split(|unit| if win32 { is_slash(*unit) } else { *unit == SLASH })
            .filter(|part| !part.is_empty())
            .map(<[u16]>::to_vec)
            .collect()
    }

    /// `nee`: node's win32 `normalize`, which every platform's path module carries.
    fn win32_normalize(&self, path: &[u16]) -> JsString {
        self.path.win32_normalize(path)
    }

    /// `Wm`: a `\??\` spelling, before or after normalizing.
    fn is_object_spelling(&self, path: &[u16]) -> bool {
        is_object_namespace(path)
            || (path.windows(2).any(|pair| is(pair, "??")) && is_object_namespace(&self.win32_normalize(path)))
    }

    /// `Pn`: a UNC or device spelling.
    fn is_unc(&self, path: &[u16]) -> bool {
        (slash_at(path, 0) && slash_at(path, 1)) || self.is_object_spelling(path)
    }

    /// `Rr`: a share automount outside Windows.
    fn is_share_automount(&self, path: &[u16]) -> bool {
        !self.win32() && share_mount(path).is_some()
    }

    /// `ud`: the lowercased share name of an automount path.
    fn share_name(&self, path: &[u16]) -> Option<JsString> {
        if self.win32() || !starts_with_slash(path) {
            return None;
        }
        // `climb` never hands over an empty stack, and a stack that names a share has at least two parts.
        climb(path, |stack| {
            names_share(stack).then(|| ascii_lower(stack.last().copied().unwrap_or_default()))
        })
    }

    /// `Wb`: whether `path` lands somewhere other than the network `anchor` the caller named.
    fn leaves_anchor(&self, path: &[u16], anchor: &[u16]) -> bool {
        if in_dot_namespace(path) {
            return true;
        }
        if self.is_share_automount(path) {
            let share = self.share_name(path);
            if share.is_none() || share != self.share_name(anchor) {
                return true;
            }
        }
        self.leaves_automount(path, anchor)
    }

    /// `MQ`.
    fn leaves_automount(&self, path: &[u16], anchor: &[u16]) -> bool {
        let home = self.darwin() && self.home_is_automounted();
        if !self.darwin() || !self.reaches_automount(path, home) {
            return false;
        }
        let (anchor_complete, _) = self.automount_points(anchor, home);
        let (complete, partial) = self.automount_points(path, home);
        if complete.is_empty() && partial.is_empty() {
            return true;
        }
        let Some(target) = anchor_complete.first() else {
            return true;
        };
        complete.iter().any(|point| point != target) || partial.iter().any(|point| !target.starts_with(point))
    }

    /// `FQ`.
    fn reaches_automount(&self, path: &[u16], home: bool) -> bool {
        if !self.darwin() {
            return false;
        }
        if on_network_mount(path) || in_dot_namespace(path) {
            return true;
        }
        if !starts_with_slash(path) {
            return false;
        }
        // Node first asks for partial points and complete `/network` points, but both only come from a path that climbs
        // through `/network`, which answered above; a `/home` point other than the user's own is all that is left.
        let (complete, _) = self.automount_points(path, home);
        let homes: Vec<&JsString> = complete.iter().filter(|point| starts_with(point, "/home/")).collect();
        !homes.is_empty() && {
            let own = self.home_folder();
            homes.iter().any(|point| own.as_ref() != Some(*point))
        }
    }

    /// `Bb`: the complete and partial automount points the path climbs through.
    fn automount_points(&self, path: &[u16], home: bool) -> (Vec<JsString>, Vec<JsString>) {
        let mut complete = Vec::new();
        let mut partial = Vec::new();
        if !starts_with_slash(path) {
            return (complete, partial);
        }
        climb(path, |stack| {
            if stack.len() > VETTING_AUTOMOUNT_DEPTH {
                return None::<()>;
            }
            let mut joined = vec![SLASH];
            joined.extend(stack.join(&SLASH));
            let mapped = if self.darwin() { firmlink_path(&joined) } else { joined };
            let parts: Vec<&[u16]> = mapped
                .split(|unit| *unit == SLASH)
                .filter(|part| !part.is_empty())
                .collect();
            let first = parts.first().map(|first| lower(first));
            if first.as_deref().is_some_and(|first| is(first, "network")) {
                match parts.as_slice() {
                    [_] => partial.push(js("/network/")),
                    [_, server] => partial.push(concat(&[&js("/network/"), &ascii_lower(server), &[SLASH]])),
                    [_, server, share] => complete.push(concat(&[
                        &js("/network/"),
                        &ascii_lower(server),
                        &[SLASH],
                        &ascii_lower(share),
                    ])),
                    _ => {}
                }
            } else if home
                && first.as_deref().is_some_and(|first| is(first, "home"))
                && let [_, user] = parts.as_slice()
            {
                complete.push(concat(&[&js("/home/"), user]));
            }
            None
        });
        (complete, partial)
    }

    /// `BI`: whether the automounter owns `/home`, remembered once it could be read.
    fn home_is_automounted(&self) -> bool {
        if let Some(known) = *guard(&self.state.home_automount) {
            return known;
        }
        match self.read_home_automount() {
            Ok(answer) => {
                *guard(&self.state.home_automount) = Some(answer);
                answer
            }
            Err(_) => true,
        }
    }

    fn read_home_automount(&self) -> Result<bool, Errno> {
        let mapped = match self.host.read_auto_master() {
            Ok(text) => text.split('\n').any(maps_home),
            Err(errno) if errno.as_deref() == Some("ENOENT") => false,
            Err(errno) => return Err(errno),
        };
        if mapped {
            return Ok(true);
        }
        self.home_is_own_mount()
    }

    /// `TI.homeIsOwnMount`.
    fn home_is_own_mount(&self) -> Result<bool, Errno> {
        let home = match self.host.stat_device(VETTING_HOME_MOUNT) {
            Ok(device) => device,
            Err(errno) if errno.as_deref() == Some("ENOENT") => return Ok(true),
            Err(errno) => return Err(errno),
        };
        Ok(home != self.host.stat_device(VETTING_USERS_MOUNT)?)
    }

    /// `HQ`: `/home/<user>` when the home directory is under `/home`.
    fn home_folder(&self) -> Option<JsString> {
        let home = self.host.home_directory().ok()?;
        let mapped = firmlink_path(&home);
        let user = non_slash_run_posix(mapped.strip_prefix(js("/home/").as_slice())?);
        (!user.is_empty()).then(|| concat(&[&js("/home/"), user]))
    }

    /// `sg`: a network anchor the path stays inside.
    fn stays_on_anchor(&self, path: &[u16], anchors: Option<&[JsString]>) -> bool {
        anchors.is_some_and(|anchors| {
            anchors.iter().any(|anchor| {
                on_network_mount(anchor)
                    && !self.leaves_anchor(path, anchor)
                    && (is_within(path, anchor) || self.is_share_automount(path))
            })
        })
    }

    /// `rM`: the path is an ancestor of a network anchor.
    fn holds_anchor(&self, path: &[u16], anchors: Option<&[JsString]>) -> bool {
        anchors.is_some_and(|anchors| {
            anchors
                .iter()
                .any(|anchor| on_network_mount(anchor) && is_within(anchor, path))
        })
    }

    /// `ag`.
    fn leaves_every_anchor(&self, path: &[u16], anchors: Option<&[JsString]>) -> bool {
        anchors.is_none_or(|anchors| anchors.iter().all(|anchor| self.leaves_anchor(path, anchor)))
    }

    /// `oM`.
    fn leaves_every_share(&self, path: &[u16], anchors: Option<&[JsString]>) -> bool {
        anchors.is_none_or(|anchors| anchors.iter().all(|anchor| self.leaves_share(path, anchor)))
    }

    /// `$I`: the UNC path is on another server than `anchor`, or cannot be compared.
    fn leaves_share(&self, path: &[u16], anchor: &[u16]) -> bool {
        if !self.is_unc(path) {
            return false;
        }
        if is_device_namespace(path) || self.is_object_spelling(path) || has_dotted_component(path) {
            return true;
        }
        let server = share_server(path);
        server.is_none() || server != share_server(anchor)
    }

    /// `ig`: where a walk that lands early ends, collapsing dotted components.
    fn landing(&self, path: &[u16], rest: &[JsString]) -> JsString {
        let dotted = has_dotted_component(path);
        if rest.is_empty() || dotted || rest.iter().any(|part| is(part, ".") || is(part, "..")) {
            return if dotted { self.collapse(path) } else { path.to_vec() };
        }
        let mut args: Vec<&[u16]> = vec![path];
        args.extend(rest.iter().map(Vec::as_slice));
        self.join(&args)
    }

    /// `Kee`.
    fn collapse(&self, path: &[u16]) -> JsString {
        if self.win32() {
            self.collapse_win32(path)
        } else {
            collapse_posix(path)
        }
    }

    /// `Bee`.
    fn collapse_win32(&self, path: &[u16]) -> JsString {
        let joined = self.join(&[path]);
        let trimmed = trim_trailing_separators(&joined);
        if self.is_unc(trimmed) && !is_wsl_share(trimmed) {
            return trimmed.to_vec();
        }
        collapse_at_dots(path)
    }

    /// `sM`: the share a composed link target lands on, if the walk should stop there.
    fn share_landing(&self, path: &[u16], anchors: Option<&[JsString]>) -> Option<JsString> {
        if self.win32() {
            return None;
        }
        let share = share_mount(path)?;
        if !self.leaves_every_anchor(&share, anchors) {
            return None;
        }
        let joined = self.join(&[path]);
        if self.is_share_automount(&joined) && self.share_name(&joined) == self.share_name(&share) {
            return Some(joined);
        }
        Some(self.landing(path, &[]))
    }

    /// `Gee`: a `..` component after any leading ones of a relative target.
    fn climbs_out(&self, text: &[u16]) -> bool {
        if self.is_absolute(text) {
            has_parent_component(text)
        } else {
            has_parent_component(strip_leading_parent_steps(text))
        }
    }

    /// `Wee`: link text the Windows walk cannot follow literally.
    fn is_opaque_link_text(&self, text: &[u16]) -> bool {
        if text.contains(&0xfffd) || self.climbs_out(text) {
            return true;
        }
        if !self.win32() {
            return false;
        }
        let drive_relative = text
            .first()
            .is_some_and(|unit| u8::try_from(*unit).is_ok_and(|byte| byte.is_ascii_alphabetic()))
            && unit_is(text, 1, ':')
            && !slash_at(text, 2);
        let rooted = slash_at(text, 0) && !slash_at(text, 1);
        let question = slash_at(text, 0) && slash_at(text, 1) && unit_is(text, 2, '?') && slash_at(text, 3);
        let plain_question = question && {
            let rest = text.get(4..).unwrap_or_default();
            let drive = rest
                .first()
                .is_some_and(|unit| u8::try_from(*unit).is_ok_and(|byte| byte.is_ascii_alphabetic()))
                && unit_is(rest, 1, ':')
                && slash_at(rest, 2);
            let unc = rest.get(..3).is_some_and(|name| eq_ascii_ci(name, "unc")) && slash_at(rest, 3);
            drive || unc
        };
        drive_relative
            || rooted
            || (question && !plain_question)
            || text
                .split(|unit| is_slash(*unit))
                .any(|part| !is(part, ".") && !is(part, "..") && ends_with_dot_or_space(part))
    }

    /// `LC`: `path` is a strict ancestor of `launch` on the same root.
    fn is_launch_ancestor(&self, path: &[u16], launch: &[u16]) -> Result<bool, NodeError> {
        let resolved = self.resolve(&[path])?;
        let launch = self.resolve(&[launch])?;
        let parts = self.components(&resolved);
        let launch_parts = self.components(&launch);
        Ok(self.root(&resolved) == self.root(&launch)
            && parts.len() < launch_parts.len()
            && parts
                .iter()
                .zip(&launch_parts)
                .all(|(part, launch_part)| part == launch_part))
    }

    /// `Nm`: the launch directory, unless this is a background session.
    fn trusted_launch(&self) -> Option<&'a [u16]> {
        let launch = self.launch?;
        let kind = self.host.env(&js(VETTING_SESSION_KIND_VARIABLE));
        if kind.is_some_and(|kind| is(trim_units(&kind), VETTING_BACKGROUND_SESSION_KIND)) {
            return None;
        }
        Some(launch)
    }

    /// `cg`: the folder the launch directory and the working directory share.
    fn launch_ancestry(&self) -> Option<JsString> {
        let launch = self.trusted_launch()?;
        let launch = self.resolve(&[launch]).ok()?;
        let cwd = self.host.cwd().ok()?;
        let cwd = self.resolve(&[&cwd]).ok()?;
        let root = self.root(&launch);
        if root != self.root(&cwd) {
            return None;
        }
        let launch_parts = self.components(launch.get(root.len()..).unwrap_or_default());
        let cwd_parts = self.components(cwd.get(root.len()..).unwrap_or_default());
        let shared = launch_parts
            .iter()
            .zip(&cwd_parts)
            .take_while(|(launch_part, cwd_part)| launch_part == cwd_part)
            .count();
        if shared == 0 {
            return None;
        }
        let mut args: Vec<&[u16]> = vec![root.as_slice()];
        args.extend(launch_parts.iter().take(shared).map(Vec::as_slice));
        Some(self.join(&args))
    }

    /// `tt`.
    fn canonical_text(&self, path: JsString) -> JsString {
        if self.darwin() {
            map_runs(&path, |run| run.nfc().collect())
        } else {
            path
        }
    }

    /// `aM`: walks `path` component by component, following links itself.
    fn walk(&self, path: &[u16], options: &WalkOptions, outcome: &mut Option<Outcome>) -> Result<Walked, NodeError> {
        let anchors = options.anchors.as_deref();
        if on_network_mount(path) && !self.stays_on_anchor(path, anchors) {
            return Ok(Walked::Path(path.to_vec()));
        }
        if (self.is_unc(path) && !is_wsl_share(path) && self.leaves_every_share(path, anchors))
            || (self.is_share_automount(path) && self.leaves_every_anchor(path, anchors))
        {
            return Ok(Walked::Done);
        }
        let sep = self.sep();
        let resolved = self.resolve(&[path])?;
        let root = self.root(&resolved);
        let mut current = root.clone();
        let mut rest = self.components(resolved.get(root.len()..).unwrap_or_default());
        let mut hops = 0;
        let mut fewest = rest.len();
        let mut through_unreadable = false;
        let mut examined = false;
        let mut seen: HashSet<JsString> = HashSet::new();
        let dotted = |rest: &[JsString]| rest.iter().any(|part| has_dotted_component(part));
        let trailing = |rest: &[JsString]| rest.first().is_some_and(|part| ends_with_dot_or_space(part));

        while let Some(head) = rest.first().cloned()
            && hops < LINK_HOP_LIMIT
        {
            if through_unreadable && has_dotted_component(&head) {
                return Ok(Walked::Unverified);
            }
            let joined = self.join(&[&current, &head]);
            if (self.is_share_automount(&joined) && self.leaves_every_anchor(&joined, anchors))
                || (on_network_mount(&joined)
                    && !(self.stays_on_anchor(&joined, anchors) || self.holds_anchor(&joined, anchors)))
            {
                return Ok(Walked::Path(self.landing(&joined, rest.get(1..).unwrap_or_default())));
            }
            if in_dot_namespace(&joined) {
                return Ok(Walked::Unverified);
            }
            let dev_fd = self.darwin() && !is(&joined, VETTING_DEV_FD) && is_within(&joined, &js(VETTING_DEV_FD));
            if dev_fd
                // `/dev/fd` is only special on macOS, where `path.dirname` is the posix one.
                && is(&self.path.posix_dirname(&joined), VETTING_DEV_FD)
                && rest.iter().skip(1).any(|part| is(part, "..") || is(part, "."))
            {
                rest.remove(0);
                while rest.len() > 1 && rest.first().is_some_and(|part| is(part, ".")) {
                    rest.remove(0);
                }
                current = joined;
                continue;
            }
            let stat = if dev_fd {
                Err(Some("ENOENT".to_owned()))
            } else {
                self.host.lstat(&joined)
            };
            let stat = match stat {
                Ok(stat) => stat,
                Err(errno) => {
                    let code = errno.as_deref();
                    if !self.win32()
                        && code.is_some_and(|code| !matches!(code, "ENOENT" | "ENOTDIR" | "ENAMETOOLONG"))
                        && self.host.open_dir_no_follow(&joined).is_ok()
                    {
                        examined = true;
                        rest.remove(0);
                        current = joined;
                        continue;
                    }
                    if let Some(launch) = options.launch_ancestry.as_deref()
                        && !launch.is_empty()
                        && !examined
                        && matches!(code, Some("EPERM" | "EACCES"))
                        && self.is_launch_ancestor(&joined, launch)?
                    {
                        through_unreadable = true;
                        rest.remove(0);
                        current = joined;
                        continue;
                    }
                    if matches!(code, Some("ENOENT" | "ENOTDIR")) {
                        *outcome = Some(Outcome::Absent(Absent {
                            at: current.clone(),
                            remaining: rest.clone(),
                            from_link_target: rest.len() > fewest,
                            not_a_folder: code == Some("ENOTDIR"),
                        }));
                    }
                    return Ok(read_failure(
                        code,
                        options.unverified,
                        hops > 0,
                        dotted(&rest),
                        trailing(&rest),
                    ));
                }
            };
            examined = true;
            if !stat.symlink {
                rest.remove(0);
                current = joined;
                continue;
            }
            hops += 1;
            let mut key = joined.clone();
            for part in &rest {
                key.push(0);
                key.extend_from_slice(part);
            }
            if !seen.insert(key) {
                return Ok(Walked::Done);
            }
            let text = match self.host.read_link(&joined) {
                Ok(text) => text,
                Err(errno) => {
                    return Ok(read_failure(
                        errno.as_deref(),
                        options.unverified,
                        hops > 0,
                        dotted(&rest),
                        trailing(&rest),
                    ));
                }
            };
            let opaque = if options.opaque_link_text {
                self.is_opaque_link_text(&text)
            } else {
                text.contains(&0xfffd)
            };
            if opaque {
                return Ok(Walked::Opaque);
            }
            let after: Vec<JsString> = rest.get(1..).unwrap_or_default().to_vec();
            if !self.is_absolute(&text) {
                let target = self.components(&text);
                let mut composed = current.clone();
                if composed.last() != Some(&sep) {
                    composed.push(sep);
                }
                let tail: Vec<&[u16]> = target.iter().chain(&after).map(Vec::as_slice).collect();
                composed.extend(tail.join(&sep));
                if let Some(landed) = self.share_landing(&composed, anchors) {
                    return Ok(Walked::Path(landed));
                }
                fewest = fewest.min(after.len());
                let mut next = target;
                next.extend(after);
                rest = next;
                continue;
            }
            if !after.is_empty() && is_net_root(&text) {
                let mut parts: Vec<&[u16]> = vec![text.as_slice()];
                parts.extend(after.iter().map(Vec::as_slice));
                if let Some(landed) = self.share_landing(&parts.join(&sep), anchors) {
                    return Ok(Walked::Path(landed));
                }
            }
            if (self.is_unc(&text) && !is_wsl_share(&text) && self.leaves_every_share(&text, anchors))
                || (self.is_share_automount(&text) && self.leaves_every_anchor(&text, anchors))
                || (on_network_mount(&text)
                    && !(self.stays_on_anchor(&text, anchors)
                        || (!after.is_empty() && self.holds_anchor(&text, anchors))))
            {
                return Ok(Walked::Path(self.landing(&text, &after)));
            }
            if has_dotted_component(&text)
                && (self.is_unc(&text) || share_mount(&text).is_some() || on_network_mount(&text) || is_net_root(&text))
            {
                if after.is_empty() {
                    return Ok(Walked::Path(text));
                }
                let mut landed = text;
                landed.push(sep);
                let tail: Vec<&[u16]> = after.iter().map(Vec::as_slice).collect();
                landed.extend(tail.join(&sep));
                return Ok(Walked::Path(landed));
            }
            let target_root = {
                let root = self.root(&text);
                if root.is_empty() { vec![sep] } else { root }
            };
            let link_root = self.root(&joined);
            if is_single_separator(&target_root) && !link_root.is_empty() && !is_single_separator(&link_root) {
                return Ok(Walked::Unverified);
            }
            let mut next = self.components(text.get(target_root.len()..).unwrap_or_default());
            current = target_root;
            examined = false;
            fewest = fewest.min(after.len());
            next.extend(after);
            rest = next;
        }
        if !rest.is_empty() && options.opaque_link_text {
            return Ok(Walked::Opaque);
        }
        if !rest.is_empty() {
            return Ok(Walked::Unverified);
        }
        if hops > 0 && is_net_root(&current) {
            return Ok(Walked::Path(current));
        }
        if on_network_mount(&current) && !self.stays_on_anchor(&current, anchors) {
            return Ok(Walked::Path(current));
        }
        *outcome = Some(Outcome::Resolved(current));
        Ok(Walked::Done)
    }

    /// `Qee`: the absolute spelling the walk starts from and the options it walks with.
    fn prepare(&self, path: &[u16]) -> Result<(JsString, WalkOptions), VetError> {
        let written = strip_device_prefix(path);
        if is_device_namespace(&written) || is_object_namespace(&written) {
            return Err(VetError::Refused(RefusalReason::Unexamined));
        }
        let resolved = if self.is_absolute(&written) {
            self.resolve(&[&written])?
        } else {
            let cwd = self.host.cwd()?;
            self.resolve(&[&cwd, &written])?
        };
        let as_written = strip_device_prefix(&resolved);
        if is_device_namespace(&as_written) || is_object_namespace(&as_written) {
            return Err(VetError::Refused(RefusalReason::Unexamined));
        }
        let own_host = on_network_mount(&as_written)
            || self.is_share_automount(&as_written)
            || (self.is_unc(&as_written) && !is_wsl_share(&as_written));
        let options = WalkOptions {
            anchors: own_host.then(|| vec![as_written.clone()]),
            unverified: !own_host,
            launch_ancestry: if own_host { None } else { self.launch_ancestry() },
            opaque_link_text: self.win32(),
        };
        Ok((as_written, options))
    }

    /// `tte`: turns what the walk returned into a refusal, or accepts it.
    fn refusal(&self, walked: &Walked, as_written: &[u16]) -> Result<(), VetError> {
        let landed = match walked {
            Walked::Done => return Ok(()),
            Walked::Unverified | Walked::Opaque => return Err(VetError::Refused(RefusalReason::Unexamined)),
            Walked::Path(landed) => landed,
        };
        if on_network_mount(as_written) && landed.as_slice() == as_written {
            return Ok(());
        }
        if !(self.is_unc(landed) || share_mount(landed).is_some() || on_network_mount(landed) || is_net_root(landed)) {
            return Err(VetError::Refused(RefusalReason::Unexamined));
        }
        Err(VetError::Refused(RefusalReason::Network))
    }

    /// `bd`: vets a path the connector check is about to read.
    pub(crate) fn vet(&self, path: &[u16]) -> Result<Vetted, VetError> {
        let (as_written, options) = self.prepare(path)?;
        let mut outcome = None;
        let walked = self.walk(&as_written, &options, &mut outcome)?;
        self.refusal(&walked, &as_written)?;
        Ok(match outcome {
            Some(Outcome::Resolved(resolved)) => Vetted {
                absent_at: None,
                links_resolved_path: Some(self.canonical_text(resolved)),
            },
            Some(Outcome::Absent(absent)) => Vetted {
                absent_at: Some(absent),
                links_resolved_path: None,
            },
            None => Vetted::default(),
        })
    }
}

/// `/^[\\/]$/`.
fn is_single_separator(root: &[u16]) -> bool {
    matches!(root, [unit] if is_slash(*unit))
}

/// `[^/]+` at the start of `text`.
fn non_slash_run_posix(text: &[u16]) -> &[u16] {
    let end = text.iter().position(|unit| *unit == SLASH).unwrap_or(text.len());
    text.get(..end).unwrap_or_default()
}
