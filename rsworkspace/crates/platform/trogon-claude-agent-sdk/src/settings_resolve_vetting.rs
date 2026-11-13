use std::{collections::HashSet, io, path::Path, sync::LazyLock};

use regex::Regex;

use crate::{
    adapters::{DirectoryEntryKind, ReadLink, ReadSymlinkMetadata},
    constants::{LINK_HOP_LIMIT, NAME_TOO_LONG_ERRNO},
    settings_resolve_sources::resolve_path,
};

/// How a path walk ended when it did not refuse the path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Vetting {
    /// Every component was examined, or the walk ended without saying more.
    Resolved,
    /// A component does not exist.
    Absent,
}

/// The walk refused the path: a component could not be examined, a link chain is too long, or a link leads onto a
/// network share or an automount path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Unvettable;

/// `fJ` upstream: format characters that render as nothing.
pub(crate) fn is_invisible(character: char) -> bool {
    matches!(character, '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}')
}

/// `Nb` upstream.
fn fold(component: &str) -> String {
    component
        .chars()
        .filter(|character| !is_invisible(*character))
        .collect::<String>()
        .to_uppercase()
        .to_lowercase()
}

/// The components of a slash path with `.` skipped and `..` popping, pushed one at a time to `visit`, which stops
/// the walk by returning `true`.
fn walk_components(path: &str, mut visit: impl FnMut(&[String]) -> bool) -> Vec<String> {
    let mut stack: Vec<String> = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            stack.pop();
            continue;
        }
        stack.push(part.to_owned());
        if visit(&stack) {
            break;
        }
    }
    stack
}

/// `dd(path) !== null` upstream: a path through an automount that reaches a network host.
fn is_automount(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut found = false;
    walk_components(path, |stack| {
        found = (stack.len() == 2 && fold(&stack[0]) == "net")
            || (stack.len() == 3 && fold(&stack[0]) == "network" && fold(&stack[1]) == "servers");
        found
    });
    found
}

/// `go` upstream: a path that passes through the top level `Network` folder.
fn is_network_root(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut found = false;
    walk_components(path, |stack| {
        found = stack.len() == 1 && stack[0].to_lowercase() == "network";
        found
    });
    found
}

/// `Qo` upstream: exactly the `/net` automount folder.
fn is_net_folder(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let stack = walk_components(path, |_| false);
    stack.len() == 1 && stack[0].to_lowercase() == "net"
}

/// `Xo` upstream: a path whose first folder is one of the `.vol`, `.file`, `.nofollow` or `.resolve` namespaces.
fn is_special_namespace(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut found = false;
    walk_components(path, |stack| {
        found = stack.len() == 1
            && [".vol", ".file", ".nofollow", ".resolve"]
                .iter()
                .any(|name| stack[0].eq_ignore_ascii_case(name));
        found
    });
    found
}

/// `Pn` upstream for a path that starts with a separator: a doubled separator or a `/??/` device prefix.
fn is_share_spelling(path: &str) -> bool {
    let mut characters = path.chars();
    let doubled = matches!(
        (characters.next(), characters.next()),
        (Some('/' | '\\'), Some('/' | '\\'))
    );
    doubled || path.starts_with("/??/") || path.starts_with("/??\\")
}

static DOT_COMPONENT: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(^|[\\/])\.{1,2}[. ]*([\\/]|$)").ok());

/// `Yn` upstream: a path with a `.` or `..` component, spelled with any run of trailing dots and spaces.
fn has_dot_component(text: &str) -> bool {
    DOT_COMPONENT.as_ref().is_some_and(|pattern| pattern.is_match(text))
}

enum Errno {
    Missing,
    Name,
    Other,
}

fn classify(error: &io::Error) -> Errno {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Errno::Missing,
        _ if error.raw_os_error() == Some(NAME_TOO_LONG_ERRNO) => Errno::Name,
        _ => Errno::Other,
    }
}

/// What `nM` upstream decides about an unreadable component.
fn unreadable(error: &io::Error, strict: bool, hops: usize, remaining: &[String]) -> Result<Vetting, Unvettable> {
    if !strict {
        return Ok(Vetting::Resolved);
    }
    match classify(error) {
        Errno::Missing => Ok(Vetting::Resolved),
        Errno::Name => {
            let dotted = remaining.iter().any(|component| has_dot_component(component));
            let spaced = remaining
                .first()
                .is_some_and(|component| component.ends_with(['.', ' ']));
            if hops == 0 && !dotted && !spaced {
                Ok(Vetting::Resolved)
            } else {
                Err(Unvettable)
            }
        }
        Errno::Other => Err(Unvettable),
    }
}

fn split_path(text: &str) -> Vec<String> {
    text.split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Walks `path` one component at a time without following anything the way the operating system would, the way
/// `bd` upstream vets a path before the SDK reads it. A symbolic link is followed by its text; a link onto a
/// network share or automount path refuses the path.
///
/// Not ported: the macOS `/dev/fd` special case, the `opendir` retry after `EACCES` on an ancestor, the launch
/// directory ancestry exemption, WSL UNC paths and Windows paths. Those answer "unexamined" here where upstream
/// may let the walk continue.
pub(crate) fn vet<H>(host: &H, path: &Path) -> Result<Vetting, Unvettable>
where
    H: ReadSymlinkMetadata<Error = io::Error> + ReadLink<Error = io::Error>,
{
    let written = resolve_path(Path::new("/"), path);
    let written_text = written.to_string_lossy().into_owned();
    let strict = !(is_network_root(&written_text) || is_automount(&written_text));
    let mut remaining = split_path(&written_text);
    let mut current = Path::new("/").to_path_buf();
    let mut hops = 0usize;
    let mut visited: HashSet<String> = HashSet::new();
    while !remaining.is_empty() && hops < LINK_HOP_LIMIT {
        let step = resolve_path(Path::new("/"), &current.join(&remaining[0]));
        let step_text = step.to_string_lossy().into_owned();
        if strict && (is_automount(&step_text) || is_network_root(&step_text)) {
            return Err(Unvettable);
        }
        if is_special_namespace(&step_text) {
            return Err(Unvettable);
        }
        let metadata = match host.symlink_metadata(&step) {
            Ok(metadata) => metadata,
            Err(error) => {
                if matches!(classify(&error), Errno::Missing) {
                    return Ok(Vetting::Absent);
                }
                return unreadable(&error, strict, hops, &remaining);
            }
        };
        if metadata.kind != DirectoryEntryKind::Symlink {
            remaining.remove(0);
            current = step;
            continue;
        }
        hops += 1;
        let key = format!("{step_text}\0{}", remaining.join("\0"));
        if !visited.insert(key) {
            return Ok(Vetting::Resolved);
        }
        let target = match host.read_link(&step) {
            Ok(target) => target,
            Err(error) => return unreadable(&error, strict, hops, &remaining),
        };
        let text = target.to_string_lossy().into_owned();
        if text.contains('\u{fffd}') {
            return Err(Unvettable);
        }
        if !text.starts_with('/') {
            let parts = split_path(&text);
            if strict {
                let base = current.to_string_lossy().into_owned();
                let base = if base.ends_with('/') { base } else { format!("{base}/") };
                let mut composed: Vec<String> = parts.clone();
                composed.extend(remaining.iter().skip(1).cloned());
                if is_automount(&format!("{base}{}", composed.join("/"))) {
                    return Err(Unvettable);
                }
            }
            remaining.remove(0);
            let mut next = parts;
            next.append(&mut remaining);
            remaining = next;
            continue;
        }
        if strict && remaining.len() > 1 && is_net_folder(&text) {
            let mut composed = vec![text.clone()];
            composed.extend(remaining.iter().skip(1).cloned());
            if is_automount(&composed.join("/")) {
                return Err(Unvettable);
            }
        }
        let network = is_share_spelling(&text)
            || (strict && (is_automount(&text) || is_network_root(&text)))
            || (has_dot_component(&text) && (is_automount(&text) || is_network_root(&text) || is_net_folder(&text)));
        if network {
            return Err(Unvettable);
        }
        remaining.remove(0);
        let mut next = split_path(&text);
        next.append(&mut remaining);
        remaining = next;
        current = Path::new("/").to_path_buf();
    }
    if !remaining.is_empty() {
        return Err(Unvettable);
    }
    let landed = current.to_string_lossy().into_owned();
    if hops > 0 && is_net_folder(&landed) {
        return Err(Unvettable);
    }
    if strict && is_network_root(&landed) {
        return Err(Unvettable);
    }
    Ok(Vetting::Resolved)
}
