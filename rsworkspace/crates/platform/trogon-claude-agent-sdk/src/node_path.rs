//! Ports node's `lib/path.js` (MIT) for the functions the connector folder check needs.
//!
//! The bundled SDK resolves paths with whatever node the host runs, so the port follows three
//! recorded releases and works on UTF-16 code units the way JS strings do. Reads of
//! `process.cwd()` and `process.env` happen at the same points, and as many times, as in node.

use crate::constants::{
    NODE_PATH_CHAR_BACKWARD_SLASH as BACKSLASH, NODE_PATH_CHAR_COLON as COLON, NODE_PATH_CHAR_DOT as DOT,
    NODE_PATH_CHAR_FORWARD_SLASH as SLASH, NODE_WINDOWS_RESERVED_NAMES,
};

/// Which node release's `path` module behavior to follow (the bundled SDK runs on whatever node the host has).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NodePathDialect {
    /// node 22.23.4 and later 22.x, 24.13.1 and later, 26.x (recorded from 24.13.1).
    #[default]
    Current,
    /// node 20.20.x (recorded from 20.20.2).
    Node20,
    /// node 22.14.0 and earlier releases without reserved device-name handling (recorded from 22.14.0).
    BeforeReservedNames,
}

impl NodePathDialect {
    /// The `.`/`?` device root check and the reserved device names arrived in the same node change.
    fn reserved_device_names(self) -> bool {
        self != Self::BeforeReservedNames
    }

    /// Only the newest releases carry the cwd fast paths, the `\\?\COM1:` case and the join special case.
    fn is_current(self) -> bool {
        self == Self::Current
    }
}

/// JS strings are UTF-16 code units; all logic works on `&[u16]`.
pub(crate) type JsString = Vec<u16>;

/// `charCodeAt` answers NaN out of range, which never equals a code, so `None` stands for it.
type Code = Option<u16>;

/// What a thrown error's `.code` is (`None` means no string code).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeError {
    pub(crate) code: Option<String>,
}

/// The slice of the node process the path functions read.
pub(crate) trait NodeProcess {
    /// `process.cwd()`; may throw.
    fn cwd(&self) -> Result<JsString, NodeError>;
    /// `process.env[key]`.
    fn env(&self, key: &[u16]) -> Option<JsString>;
}

/// The object `path.parse` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParsedPath {
    pub(crate) root: JsString,
    pub(crate) dir: JsString,
    pub(crate) base: JsString,
    pub(crate) ext: JsString,
    pub(crate) name: JsString,
}

/// node's `path` module for one release and one host.
pub(crate) struct NodePath<'a> {
    pub(crate) dialect: NodePathDialect,
    /// node's `isWindows` (`process.platform === "win32"`).
    pub(crate) windows_host: bool,
    pub(crate) process: &'a dyn NodeProcess,
}

pub(crate) fn js(text: &str) -> JsString {
    text.encode_utf16().collect()
}

pub(crate) fn rust(text: &[u16]) -> String {
    String::from_utf16_lossy(text)
}

fn len(s: &[u16]) -> i64 {
    i64::try_from(s.len()).unwrap_or(i64::MAX)
}

fn at(s: &[u16], index: i64) -> Code {
    usize::try_from(index).ok().and_then(|index| s.get(index)).copied()
}

fn clamp_index(index: i64, length: i64) -> i64 {
    if index < 0 {
        index.saturating_add(length).max(0)
    } else {
        index.min(length)
    }
}

/// `String.prototype.slice`, including negative indexes.
fn slice(s: &[u16], start: i64, end: i64) -> &[u16] {
    let length = len(s);
    // Both ends are clamped into `0..=length`, so they always fit; an end before the start gives nothing.
    let from = usize::try_from(clamp_index(start, length)).unwrap_or_default();
    let to = usize::try_from(clamp_index(end, length)).unwrap_or_default();
    s.get(from..to.max(from)).unwrap_or_default()
}

fn slice_from(s: &[u16], start: i64) -> &[u16] {
    slice(s, start, i64::MAX)
}

/// `String.prototype.indexOf` for one code unit.
fn index_of(s: &[u16], needle: u16, from: i64) -> i64 {
    let skip = usize::try_from(from.max(0)).unwrap_or(usize::MAX);
    s.iter()
        .enumerate()
        .skip(skip)
        .find(|(_, unit)| **unit == needle)
        .and_then(|(index, _)| i64::try_from(index).ok())
        .unwrap_or(-1)
}

/// `String.prototype.lastIndexOf` for one code unit.
fn last_index_of(s: &[u16], needle: u16) -> i64 {
    s.iter()
        .rposition(|unit| *unit == needle)
        .and_then(|index| i64::try_from(index).ok())
        .unwrap_or(-1)
}

fn eq_str(s: &[u16], text: &str) -> bool {
    s.iter().copied().eq(text.encode_utf16())
}

/// `toLowerCase` keeps lone surrogates, and a lone surrogate ends the final-sigma context.
fn lower(s: &[u16]) -> JsString {
    fn flush(run: &mut String, out: &mut JsString) {
        out.extend(run.to_lowercase().encode_utf16());
        run.clear();
    }
    let mut out = JsString::with_capacity(s.len());
    let mut run = String::new();
    for unit in char::decode_utf16(s.iter().copied()) {
        match unit {
            Ok(c) => run.push(c),
            Err(lone) => {
                flush(&mut run, &mut out);
                out.push(lone.unpaired_surrogate());
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

fn is_path_separator(code: Code) -> bool {
    code == Some(SLASH) || code == Some(BACKSLASH)
}

fn is_posix_path_separator(code: Code) -> bool {
    code == Some(SLASH)
}

fn is_windows_device_root(code: Code) -> bool {
    code.is_some_and(|unit| {
        (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) || (u16::from(b'a')..=u16::from(b'z')).contains(&unit)
    })
}

/// A lone surrogate cannot match a reserved name, so the lossy decode never changes the answer.
fn is_windows_reserved_name(path: &[u16], colon_index: i64) -> bool {
    let device = rust(slice(path, 0, colon_index)).to_uppercase();
    NODE_WINDOWS_RESERVED_NAMES.contains(&device.as_str())
}

/// Skips code units that are (`separators`) or are not path separators, starting at `from`.
fn scan(path: &[u16], from: i64, separators: bool) -> i64 {
    let mut j = from;
    while j < len(path) && is_path_separator(at(path, j)) == separators {
        j += 1;
    }
    j
}

fn join_with(parts: &[&[u16]], separator: u16) -> JsString {
    let mut out = JsString::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.push(separator);
        }
        out.extend_from_slice(part);
    }
    out
}

/// Resolves `.` and `..` elements in a path with directory names.
fn normalize_string(
    path: &[u16],
    allow_above_root: bool,
    separator: u16,
    is_separator: fn(Code) -> bool,
    dialect: NodePathDialect,
) -> JsString {
    let mut res = JsString::new();
    let mut last_segment_length: i64 = 0;
    let mut last_slash: i64 = -1;
    let mut dots: i64 = 0;
    let mut code: Code = Some(0);
    for i in 0..=len(path) {
        if i < len(path) {
            code = at(path, i);
        } else if is_separator(code) {
            break;
        } else {
            code = Some(SLASH);
        }

        if is_separator(code) {
            if last_slash == i - 1 || dots == 1 {
                // NOOP
            } else if dots == 2 {
                let res_len = len(&res);
                if res_len < 2
                    || last_segment_length != 2
                    || at(&res, res_len - 1) != Some(DOT)
                    || at(&res, res_len - 2) != Some(DOT)
                {
                    if res_len > 2 {
                        // Releases before 22.23.4 and 24.13.1 search for the separator instead.
                        let last_slash_index = if dialect.is_current() {
                            res_len - last_segment_length - 1
                        } else {
                            last_index_of(&res, separator)
                        };
                        if last_slash_index == -1 {
                            res.clear();
                            last_segment_length = 0;
                        } else {
                            res = slice(&res, 0, last_slash_index).to_vec();
                            last_segment_length = len(&res) - 1 - last_index_of(&res, separator);
                        }
                        last_slash = i;
                        dots = 0;
                        continue;
                    } else if res_len != 0 {
                        res.clear();
                        last_segment_length = 0;
                        last_slash = i;
                        dots = 0;
                        continue;
                    }
                }
                if allow_above_root {
                    if !res.is_empty() {
                        res.push(separator);
                    }
                    res.push(DOT);
                    res.push(DOT);
                    last_segment_length = 2;
                }
            } else {
                let segment = slice(path, last_slash + 1, i);
                if res.is_empty() {
                    res = segment.to_vec();
                } else {
                    res.push(separator);
                    res.extend_from_slice(segment);
                }
                last_segment_length = i - last_slash - 1;
            }
            last_slash = i;
            dots = 0;
        } else if code == Some(DOT) && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
    }
    res
}

/// `\\first\rest`.
fn unc_device(first: &[u16], rest: &[u16]) -> JsString {
    [&[BACKSLASH, BACKSLASH][..], first, &[BACKSLASH], rest].concat()
}

/// `\\first`.
fn device_root(first: &[u16]) -> JsString {
    [&[BACKSLASH, BACKSLASH][..], first].concat()
}

fn is_dot_or_question(first: &[u16]) -> bool {
    eq_str(first, ".") || eq_str(first, "?")
}

impl NodePath<'_> {
    /// `posixCwd`: on a Windows host node turns the Windows cwd into a POSIX looking one.
    fn posix_cwd(&self) -> Result<JsString, NodeError> {
        let cwd = self.process.cwd()?;
        if !self.windows_host {
            return Ok(cwd);
        }
        let converted: JsString = cwd
            .iter()
            .map(|unit| if *unit == BACKSLASH { SLASH } else { *unit })
            .collect();
        // indexOf may answer -1, and slice(-1) then keeps the last code unit.
        let first_slash = index_of(&converted, SLASH, 0);
        Ok(slice_from(&converted, first_slash).to_vec())
    }

    pub(crate) fn posix_resolve(&self, args: &[&[u16]]) -> Result<JsString, NodeError> {
        let current_dir_only = match args {
            [] => true,
            [only] => only.is_empty() || eq_str(only, "."),
            _ => false,
        };
        if self.dialect.is_current() && current_dir_only {
            let cwd = self.posix_cwd()?;
            if at(&cwd, 0) == Some(SLASH) {
                return Ok(cwd);
            }
        }
        let mut resolved_path = JsString::new();
        let mut resolved_absolute = false;

        for path in args.iter().rev().copied() {
            if resolved_absolute {
                break;
            }
            if path.is_empty() {
                continue;
            }
            resolved_path = [path, &[SLASH][..], &resolved_path].concat();
            resolved_absolute = at(path, 0) == Some(SLASH);
        }

        if !resolved_absolute {
            let cwd = self.posix_cwd()?;
            resolved_path = [&cwd[..], &[SLASH][..], &resolved_path].concat();
            resolved_absolute = at(&cwd, 0) == Some(SLASH);
        }

        let normalized = normalize_string(
            &resolved_path,
            !resolved_absolute,
            SLASH,
            is_posix_path_separator,
            self.dialect,
        );
        if resolved_absolute {
            return Ok([&[SLASH][..], &normalized].concat());
        }
        Ok(if normalized.is_empty() { js(".") } else { normalized })
    }

    pub(crate) fn posix_normalize(&self, path: &[u16]) -> JsString {
        if path.is_empty() {
            return js(".");
        }
        let is_absolute = at(path, 0) == Some(SLASH);
        let trailing_separator = at(path, len(path) - 1) == Some(SLASH);

        let mut normalized = normalize_string(path, !is_absolute, SLASH, is_posix_path_separator, self.dialect);
        if normalized.is_empty() {
            if is_absolute {
                return js("/");
            }
            return js(if trailing_separator { "./" } else { "." });
        }
        if trailing_separator {
            normalized.push(SLASH);
        }
        if is_absolute {
            [&[SLASH][..], &normalized].concat()
        } else {
            normalized
        }
    }

    pub(crate) fn posix_join(&self, args: &[&[u16]]) -> JsString {
        let parts: Vec<&[u16]> = args.iter().copied().filter(|arg| !arg.is_empty()).collect();
        if parts.is_empty() {
            return js(".");
        }
        self.posix_normalize(&join_with(&parts, SLASH))
    }

    pub(crate) fn posix_dirname(&self, path: &[u16]) -> JsString {
        if path.is_empty() {
            return js(".");
        }
        let has_root = at(path, 0) == Some(SLASH);
        let mut end = -1;
        let mut matched_slash = true;
        for i in (1..len(path)).rev() {
            if at(path, i) == Some(SLASH) {
                if !matched_slash {
                    end = i;
                    break;
                }
            } else {
                matched_slash = false;
            }
        }

        if end == -1 {
            return js(if has_root { "/" } else { "." });
        }
        if has_root && end == 1 {
            return js("//");
        }
        slice(path, 0, end).to_vec()
    }

    pub(crate) fn posix_is_absolute(&self, path: &[u16]) -> bool {
        at(path, 0) == Some(SLASH)
    }

    pub(crate) fn posix_parse(&self, path: &[u16]) -> ParsedPath {
        let mut ret = ParsedPath::default();
        if path.is_empty() {
            return ret;
        }
        let is_absolute = at(path, 0) == Some(SLASH);
        let start = if is_absolute {
            ret.root = js("/");
            1
        } else {
            0
        };
        let mut start_dot = -1;
        let mut start_part = 0;
        let mut end = -1;
        let mut matched_slash = true;
        let mut pre_dot_state = 0;

        for i in (start..len(path)).rev() {
            let code = at(path, i);
            if code == Some(SLASH) {
                if !matched_slash {
                    start_part = i + 1;
                    break;
                }
                continue;
            }
            if end == -1 {
                matched_slash = false;
                end = i + 1;
            }
            if code == Some(DOT) {
                if start_dot == -1 {
                    start_dot = i;
                } else if pre_dot_state != 1 {
                    pre_dot_state = 1;
                }
            } else if start_dot != -1 {
                pre_dot_state = -1;
            }
        }

        if end != -1 {
            let from = if start_part == 0 && is_absolute { 1 } else { start_part };
            if start_dot == -1
                || pre_dot_state == 0
                || (pre_dot_state == 1 && start_dot == end - 1 && start_dot == start_part + 1)
            {
                ret.base = slice(path, from, end).to_vec();
                ret.name = ret.base.clone();
            } else {
                ret.name = slice(path, from, start_dot).to_vec();
                ret.base = slice(path, from, end).to_vec();
                ret.ext = slice(path, start_dot, end).to_vec();
            }
        }

        if start_part > 0 {
            ret.dir = slice(path, 0, start_part - 1).to_vec();
        } else if is_absolute {
            ret.dir = js("/");
        }
        ret
    }

    pub(crate) fn win32_resolve(&self, args: &[&[u16]]) -> Result<JsString, NodeError> {
        let mut resolved_device = JsString::new();
        let mut resolved_tail = JsString::new();
        let mut resolved_absolute = false;

        // The trailing None is node's `i === -1` turn, which asks for the cwd.
        for entry in args.iter().rev().copied().map(Some).chain(std::iter::once(None)) {
            let path: JsString = match entry {
                Some(arg) => {
                    if arg.is_empty() {
                        continue;
                    }
                    arg.to_vec()
                }
                None if resolved_device.is_empty() => {
                    let cwd = self.process.cwd()?;
                    let current_dir_only = match args {
                        [] => true,
                        [only] => (only.is_empty() || eq_str(only, ".")) && is_path_separator(at(&cwd, 0)),
                        _ => false,
                    };
                    if self.dialect.is_current() && current_dir_only {
                        if self.windows_host {
                            return Ok(cwd);
                        }
                        return Ok(cwd
                            .iter()
                            .map(|unit| if *unit == SLASH { BACKSLASH } else { *unit })
                            .collect());
                    }
                    cwd
                }
                None => {
                    // Windows keeps a current directory per drive, kept in `=C:` style variables.
                    let key = [&[u16::from(b'=')][..], &resolved_device].concat();
                    let found = match self.process.env(&key) {
                        Some(value) if !value.is_empty() => value,
                        _ => self.process.cwd()?,
                    };
                    if lower(slice(&found, 0, 2)) != lower(&resolved_device) && at(&found, 2) == Some(BACKSLASH) {
                        [&resolved_device[..], &[BACKSLASH]].concat()
                    } else {
                        found
                    }
                }
            };

            let path_len = len(&path);
            let mut root_end = 0;
            let mut device = JsString::new();
            let mut is_absolute = false;
            let code = at(&path, 0);

            if path_len == 1 {
                if is_path_separator(code) {
                    root_end = 1;
                    is_absolute = true;
                }
            } else if is_path_separator(code) {
                is_absolute = true;

                if is_path_separator(at(&path, 1)) {
                    let mut last = 2;
                    let mut j = scan(&path, last, false);
                    if j < path_len && j != last {
                        let first_part = slice(&path, last, j);
                        last = j;
                        j = scan(&path, j, true);
                        if j < path_len && j != last {
                            last = j;
                            j = scan(&path, j, false);
                            // Node also checks `j === len || j !== last` here, which always holds:
                            // the name scan starts on a unit that is not a separator, so it moves at
                            // least once.
                            if self.dialect.reserved_device_names() && is_dot_or_question(first_part) {
                                device = device_root(first_part);
                                root_end = 4;
                            } else {
                                device = unc_device(first_part, slice(&path, last, j));
                                root_end = j;
                            }
                        }
                    }
                } else {
                    root_end = 1;
                }
            } else if is_windows_device_root(code) && at(&path, 1) == Some(COLON) {
                device = slice(&path, 0, 2).to_vec();
                root_end = 2;
                if path_len > 2 && is_path_separator(at(&path, 2)) {
                    is_absolute = true;
                    root_end = 3;
                }
            }

            if !device.is_empty() {
                if resolved_device.is_empty() {
                    resolved_device = device;
                } else if lower(&device) != lower(&resolved_device) {
                    // This path points to another device so it is not applicable.
                    continue;
                }
            }

            if resolved_absolute {
                if !resolved_device.is_empty() {
                    break;
                }
            } else {
                resolved_tail = [slice_from(&path, root_end), &[BACKSLASH][..], &resolved_tail].concat();
                resolved_absolute = is_absolute;
                if is_absolute && !resolved_device.is_empty() {
                    break;
                }
            }
        }

        let tail = normalize_string(
            &resolved_tail,
            !resolved_absolute,
            BACKSLASH,
            is_path_separator,
            self.dialect,
        );
        if resolved_absolute {
            return Ok([&resolved_device[..], &[BACKSLASH], &tail].concat());
        }
        let relative = [&resolved_device[..], &tail].concat();
        Ok(if relative.is_empty() { js(".") } else { relative })
    }

    pub(crate) fn win32_normalize(&self, path: &[u16]) -> JsString {
        let path_len = len(path);
        if path_len == 0 {
            return js(".");
        }
        let mut root_end = 0;
        let mut device: Option<JsString> = None;
        let mut is_absolute = false;
        let code = at(path, 0);

        if path_len == 1 {
            return if is_posix_path_separator(code) {
                vec![BACKSLASH]
            } else {
                path.to_vec()
            };
        }
        if is_path_separator(code) {
            is_absolute = true;

            if is_path_separator(at(path, 1)) {
                let mut last = 2;
                let mut j = scan(path, last, false);
                if j < path_len && j != last {
                    let first_part = slice(path, last, j);
                    last = j;
                    j = scan(path, j, true);
                    if j < path_len && j != last {
                        last = j;
                        j = scan(path, j, false);
                        // Node also checks `j === len || j !== last` here, which always holds:
                        // the name scan starts on a unit that is not a separator, so it moves at
                        // least once.
                        if self.dialect.reserved_device_names() && is_dot_or_question(first_part) {
                            device = Some(device_root(first_part));
                            root_end = 4;
                            if self.dialect.is_current() {
                                // Reserved device paths such as `\\?\COM1:` keep their colon.
                                let colon_index = index_of(path, COLON, 0);
                                let possible_device = slice(path, 4, colon_index + 1);
                                if is_windows_reserved_name(possible_device, len(possible_device) - 1) {
                                    device = Some(
                                        [&[BACKSLASH, BACKSLASH, u16::from(b'?'), BACKSLASH][..], possible_device]
                                            .concat(),
                                    );
                                    root_end = 4 + len(possible_device);
                                }
                            }
                        } else if j == path_len {
                            // Only a UNC root is left, so there is nothing more to process.
                            return [
                                &[BACKSLASH, BACKSLASH][..],
                                first_part,
                                &[BACKSLASH],
                                slice_from(path, last),
                                &[BACKSLASH],
                            ]
                            .concat();
                        } else {
                            device = Some(unc_device(first_part, slice(path, last, j)));
                            root_end = j;
                        }
                    }
                }
            } else {
                root_end = 1;
            }
        } else if is_windows_device_root(code) && at(path, 1) == Some(COLON) {
            device = Some(slice(path, 0, 2).to_vec());
            root_end = 2;
            if path_len > 2 && is_path_separator(at(path, 2)) {
                is_absolute = true;
                root_end = 3;
            }
        } else if self.dialect.reserved_device_names() {
            let colon_index = index_of(path, COLON, 0);
            if colon_index > 0 && is_windows_reserved_name(path, colon_index) {
                device = Some(slice(path, 0, colon_index + 1).to_vec());
                root_end = colon_index + 1;
            }
        }

        let mut tail = if root_end < path_len {
            normalize_string(
                slice_from(path, root_end),
                !is_absolute,
                BACKSLASH,
                is_path_separator,
                self.dialect,
            )
        } else {
            JsString::new()
        };
        if tail.is_empty() && !is_absolute {
            tail = js(".");
        }
        if !tail.is_empty() && is_path_separator(at(path, path_len - 1)) {
            tail.push(BACKSLASH);
        }
        if !is_absolute && device.is_none() && path.contains(&COLON) {
            // A relative path must not turn into something Windows reads as absolute (CVE-2024-36139).
            if tail.len() >= 2 && is_windows_device_root(at(&tail, 0)) && at(&tail, 1) == Some(COLON) {
                return [&[DOT, BACKSLASH][..], &tail].concat();
            }
            let mut index = index_of(path, COLON, 0);
            loop {
                if index == path_len - 1 || is_path_separator(at(path, index + 1)) {
                    return [&[DOT, BACKSLASH][..], &tail].concat();
                }
                index = index_of(path, COLON, index + 1);
                if index == -1 {
                    break;
                }
            }
        }
        if self.dialect.reserved_device_names() {
            let colon_index = index_of(path, COLON, 0);
            // With no colon this slices off the last code unit, which is how node behaves.
            if is_windows_reserved_name(path, colon_index) {
                return [&[DOT, BACKSLASH][..], device.as_deref().unwrap_or(&[]), &tail].concat();
            }
        }
        match device {
            None if is_absolute => [&[BACKSLASH][..], &tail].concat(),
            None => tail,
            Some(device) if is_absolute => [&device[..], &[BACKSLASH], &tail].concat(),
            Some(device) => [&device[..], &tail].concat(),
        }
    }

    pub(crate) fn win32_is_absolute(&self, path: &[u16]) -> bool {
        let path_len = len(path);
        if path_len == 0 {
            return false;
        }
        let code = at(path, 0);
        is_path_separator(code)
            || (path_len > 2
                && is_windows_device_root(code)
                && at(path, 1) == Some(COLON)
                && is_path_separator(at(path, 2)))
    }

    pub(crate) fn win32_join(&self, args: &[&[u16]]) -> JsString {
        let parts: Vec<&[u16]> = args.iter().copied().filter(|arg| !arg.is_empty()).collect();
        let Some(first_part) = parts.first().copied() else {
            return js(".");
        };
        let mut joined = join_with(&parts, BACKSLASH);

        // A joined path starting with two slashes would be mistaken for a UNC path by normalize,
        // unless the first part clearly meant one: exactly two slashes and then a name.
        let mut needs_replace = true;
        let mut slash_count: i64 = 0;
        if is_path_separator(at(first_part, 0)) {
            slash_count += 1;
            let first_len = len(first_part);
            if first_len > 1 && is_path_separator(at(first_part, 1)) {
                slash_count += 1;
                if first_len > 2 {
                    if is_path_separator(at(first_part, 2)) {
                        slash_count += 1;
                    } else {
                        needs_replace = false;
                    }
                }
            }
        }
        if needs_replace {
            while slash_count < len(&joined) && is_path_separator(at(&joined, slash_count)) {
                slash_count += 1;
            }
            if slash_count >= 2 {
                joined = [&[BACKSLASH][..], slice_from(&joined, slash_count)].concat();
            }
        }

        if self.dialect.is_current() {
            let has_reserved_part = joined
                .split(|unit| *unit == BACKSLASH)
                .filter(|part| !part.is_empty())
                .any(|part| {
                    let colon_index = index_of(part, COLON, 0);
                    colon_index != -1 && is_windows_reserved_name(part, colon_index)
                });
            if has_reserved_part {
                // Skip normalization when a reserved device name is present.
                return joined
                    .iter()
                    .map(|unit| if *unit == SLASH { BACKSLASH } else { *unit })
                    .collect();
            }
        }

        self.win32_normalize(&joined)
    }

    pub(crate) fn win32_dirname(&self, path: &[u16]) -> JsString {
        let path_len = len(path);
        if path_len == 0 {
            return js(".");
        }
        let mut root_end: i64 = -1;
        let mut offset: i64 = 0;
        let code = at(path, 0);

        if path_len == 1 {
            return if is_path_separator(code) {
                path.to_vec()
            } else {
                js(".")
            };
        }

        if is_path_separator(code) {
            root_end = 1;
            offset = 1;

            if is_path_separator(at(path, 1)) {
                let mut last = 2;
                let mut j = scan(path, last, false);
                if j < path_len && j != last {
                    last = j;
                    j = scan(path, j, true);
                    if j < path_len && j != last {
                        last = j;
                        j = scan(path, j, false);
                        if j == path_len {
                            // Only a UNC root.
                            return path.to_vec();
                        }
                        if j != last {
                            // Include the separator after the UNC root, treating it as a normal root.
                            root_end = j + 1;
                            offset = j + 1;
                        }
                    }
                }
            }
        } else if is_windows_device_root(code) && at(path, 1) == Some(COLON) {
            root_end = if path_len > 2 && is_path_separator(at(path, 2)) {
                3
            } else {
                2
            };
            offset = root_end;
        }

        let mut end = -1;
        let mut matched_slash = true;
        for i in (offset..path_len).rev() {
            if is_path_separator(at(path, i)) {
                if !matched_slash {
                    end = i;
                    break;
                }
            } else {
                matched_slash = false;
            }
        }

        if end == -1 {
            if root_end == -1 {
                return js(".");
            }
            end = root_end;
        }
        slice(path, 0, end).to_vec()
    }

    pub(crate) fn win32_parse(&self, path: &[u16]) -> ParsedPath {
        let mut ret = ParsedPath::default();
        let path_len = len(path);
        if path_len == 0 {
            return ret;
        }
        let mut root_end = 0;
        let code = at(path, 0);

        if path_len == 1 {
            if is_path_separator(code) {
                ret.root = path.to_vec();
                ret.dir = path.to_vec();
                return ret;
            }
            ret.base = path.to_vec();
            ret.name = path.to_vec();
            return ret;
        }
        if is_path_separator(code) {
            root_end = 1;
            if is_path_separator(at(path, 1)) {
                let mut last = 2;
                let mut j = scan(path, last, false);
                if j < path_len && j != last {
                    last = j;
                    j = scan(path, j, true);
                    if j < path_len && j != last {
                        last = j;
                        j = scan(path, j, false);
                        if j == path_len {
                            root_end = j;
                        } else if j != last {
                            root_end = j + 1;
                        }
                    }
                }
            }
        } else if is_windows_device_root(code) && at(path, 1) == Some(COLON) {
            if path_len <= 2 {
                ret.root = path.to_vec();
                ret.dir = path.to_vec();
                return ret;
            }
            root_end = 2;
            if is_path_separator(at(path, 2)) {
                if path_len == 3 {
                    ret.root = path.to_vec();
                    ret.dir = path.to_vec();
                    return ret;
                }
                root_end = 3;
            }
        }
        if root_end > 0 {
            ret.root = slice(path, 0, root_end).to_vec();
        }

        let mut start_dot = -1;
        let mut start_part = root_end;
        let mut end = -1;
        let mut matched_slash = true;
        let mut pre_dot_state = 0;

        for i in (root_end..path_len).rev() {
            let code = at(path, i);
            if is_path_separator(code) {
                if !matched_slash {
                    start_part = i + 1;
                    break;
                }
                continue;
            }
            if end == -1 {
                matched_slash = false;
                end = i + 1;
            }
            if code == Some(DOT) {
                if start_dot == -1 {
                    start_dot = i;
                } else if pre_dot_state != 1 {
                    pre_dot_state = 1;
                }
            } else if start_dot != -1 {
                pre_dot_state = -1;
            }
        }

        if end != -1 {
            if start_dot == -1
                || pre_dot_state == 0
                || (pre_dot_state == 1 && start_dot == end - 1 && start_dot == start_part + 1)
            {
                ret.base = slice(path, start_part, end).to_vec();
                ret.name = ret.base.clone();
            } else {
                ret.name = slice(path, start_part, start_dot).to_vec();
                ret.base = slice(path, start_part, end).to_vec();
                ret.ext = slice(path, start_dot, end).to_vec();
            }
        }

        // The root keeps its trailing separator as `dir` (`C:\abc` is `C:\`), other dirs lose it.
        if start_part > 0 && start_part != root_end {
            ret.dir = slice(path, 0, start_part - 1).to_vec();
        } else {
            ret.dir = ret.root.clone();
        }
        ret
    }
}

#[cfg(test)]
#[path = "node_path_tests.rs"]
mod tests;
