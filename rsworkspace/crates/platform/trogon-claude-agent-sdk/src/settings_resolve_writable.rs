//! Whether the user running the SDK could rewrite a managed settings document (`GP` upstream). A cloud session that
//! takes its managed settings from the organization lets such a document only restrict.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::{
    adapters::{DirectoryEntryKind, SettingsHost},
    constants::{
        MODE_GROUP_WRITABLE, MODE_OTHER_WRITABLE, WRITABLE_WALK_LINK_LIMIT, WSL_MANAGED_DIRECTORY, WSL_REGISTRY_TOOL,
    },
    settings_resolve_platform::PlatformKind,
    settings_resolve_sources::resolve_path,
    settings_resolve_vetting::{Vetting, vet},
};

/// The owner, group and mode bits a `stat` reports (`fileOwner` upstream).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Owner {
    uid: u32,
    gid: u32,
    mode: u32,
}

#[cfg(unix)]
fn owner_of(metadata: &std::fs::Metadata) -> Option<Owner> {
    Some(Owner {
        uid: metadata.uid(),
        gid: metadata.gid(),
        mode: metadata.mode(),
    })
}

#[cfg(not(unix))]
fn owner_of(_metadata: &std::fs::Metadata) -> Option<Owner> {
    None
}

fn stat_owner<H: SettingsHost>(host: &H, path: &Path) -> Option<Owner> {
    host.metadata(path).ok().as_ref().and_then(owner_of)
}

fn is_link<H: SettingsHost>(host: &H, path: &Path) -> io::Result<bool> {
    host.symlink_metadata(path)
        .map(|metadata| metadata.kind == DirectoryEntryKind::Symlink)
}

/// One managed document the resolver read: `root` joined with `components`, the way `um` upstream names it.
pub(crate) struct DocumentProbe<'a> {
    pub(crate) root: &'a Path,
    pub(crate) components: Vec<String>,
    /// What the folder listing said about the last component (`true` for a symbolic link), when it was listed.
    pub(crate) last_is_link: Option<bool>,
    /// The read opened the document, so upstream holds its owner.
    pub(crate) opened: bool,
}

impl DocumentProbe<'_> {
    fn path(&self) -> PathBuf {
        let mut path = self.root.to_path_buf();
        for component in &self.components {
            path.push(component);
        }
        path
    }
}

/// The ids `A4` upstream checks owners against.
struct Identity {
    uids: Vec<u32>,
    groups: Vec<u32>,
}

fn identity<H: SettingsHost>(host: &H, euid: u32, uid: Option<u32>) -> Identity {
    let uids = match uid {
        Some(uid) if uid != euid => vec![euid, uid],
        _ => vec![euid],
    };
    let mut groups = host.supplementary_group_ids().and_then(Result::ok).unwrap_or_default();
    groups.extend(host.effective_group_id());
    groups.extend(host.real_group_id());
    Identity { uids, groups }
}

/// `qD` upstream: `Some(true)` when an owner lets `euid` write, `None` when the answer needs both owners and one
/// is unknown.
fn owners_allow(file: Option<Owner>, folder: Option<Owner>, euid: u32, groups: &[u32]) -> Option<bool> {
    if euid == 0 {
        return Some(true);
    }
    let owners: Vec<Owner> = [file, folder].into_iter().flatten().collect();
    let allows = owners.iter().any(|owner| {
        owner.uid == euid
            || owner.mode & MODE_OTHER_WRITABLE != 0
            || (owner.mode & MODE_GROUP_WRITABLE != 0 && groups.contains(&owner.gid))
    });
    if allows {
        Some(true)
    } else if owners.len() < 2 {
        None
    } else {
        Some(false)
    }
}

/// `Hx` upstream: the first answer for any of the user ids that is not a plain no.
fn identity_allows(identity: &Identity, file: Option<Owner>, folder: Option<Owner>) -> Option<bool> {
    for uid in &identity.uids {
        let answer = owners_allow(file, folder, *uid, &identity.groups);
        if answer != Some(false) {
            return answer;
        }
    }
    Some(false)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriteCheck {
    Yes,
    No,
    Unknown,
}

/// `Fx` upstream: `access(W_OK)`, where only `EACCES`, `EPERM` and `EROFS` say no.
fn write_check<H: SettingsHost>(host: &H, path: &Path) -> WriteCheck {
    match host.check_writable(path) {
        Ok(()) => WriteCheck::Yes,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            WriteCheck::No
        }
        Err(_) => WriteCheck::Unknown,
    }
}

/// How a document sits among symbolic links (`linkOf` upstream), as far as the write check tells the cases apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Linkage {
    /// No link on the way (`null`).
    Direct,
    /// A link on the way whose ends were found (`inside` or `outside`).
    Linked,
    /// A link on the way whose ends could not be examined (`unverified`).
    Unverified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Settled {
    Gone,
    Unverified,
    Found,
}

/// `Lx` upstream: whether `path` can be followed to where it really is.
fn settle<H: SettingsHost>(host: &H, path: &Path) -> Settled {
    match vet(host, path) {
        Ok(Vetting::Resolved) => Settled::Found,
        Ok(Vetting::Absent) => Settled::Gone,
        Err(_) => Settled::Unverified,
    }
}

/// `E4` upstream.
fn linkage<H: SettingsHost>(host: &H, probe: &DocumentProbe<'_>) -> Linkage {
    let root = resolve_path(Path::new("/"), probe.root);
    let Ok(root_linked) = is_link(host, &root) else {
        return Linkage::Direct;
    };
    if root_linked {
        return Linkage::Linked;
    }
    let mut current = probe.root.to_path_buf();
    let mut linked = false;
    for (index, component) in probe.components.iter().enumerate() {
        current.push(component);
        let answer = match probe.last_is_link {
            Some(known) if index + 1 == probe.components.len() => Ok(known),
            _ => is_link(host, &current),
        };
        let Ok(answer) = answer else {
            return Linkage::Direct;
        };
        linked = answer;
        if linked {
            break;
        }
    }
    if !linked {
        return Linkage::Direct;
    }
    let document = settle(host, &probe.path());
    let folder = settle(host, probe.root);
    if document == Settled::Gone || folder == Settled::Gone {
        return Linkage::Direct;
    }
    if document == Settled::Unverified || folder == Settled::Unverified {
        return Linkage::Unverified;
    }
    Linkage::Linked
}

/// The owner of the folder that holds the document, known only when no folder on the way from `root` is a link.
fn folder_owner<H: SettingsHost>(host: &H, probe: &DocumentProbe<'_>) -> Option<Owner> {
    let mut folder = resolve_path(Path::new("/"), probe.root);
    if is_link(host, &folder).ok()? {
        return None;
    }
    let folders = probe.components.len().saturating_sub(1);
    for component in probe.components.iter().take(folders) {
        folder.push(component);
        if is_link(host, &folder).ok()? {
            return None;
        }
    }
    stat_owner(host, &folder)
}

enum Walk {
    Answer(bool),
    Follow(PathBuf),
}

/// One pass of `R4` upstream: from the root down, a folder the user could write ends the walk with yes, and a link
/// starts the walk over from where it points.
fn walk_once<H: SettingsHost>(
    host: &H,
    path: &Path,
    identity: &Identity,
    certain: &mut HashMap<PathBuf, bool>,
) -> Walk {
    let text = path.to_string_lossy().into_owned();
    let parts: Vec<&str> = text
        .strip_prefix('/')
        .unwrap_or(&text)
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let mut folder = PathBuf::from("/");
    for (index, part) in parts.iter().enumerate() {
        let sealed = match certain.get(&folder) {
            Some(sealed) => *sealed,
            None => {
                let Some(owner) = stat_owner(host, &folder) else {
                    return Walk::Answer(true);
                };
                let sealed = identity_allows(identity, Some(owner), Some(owner)) == Some(false)
                    && write_check(host, &folder) == WriteCheck::No;
                certain.insert(folder.clone(), sealed);
                sealed
            }
        };
        if !sealed {
            return Walk::Answer(true);
        }
        let next = folder.join(part);
        if certain.contains_key(&next) {
            folder = next;
            continue;
        }
        let Ok(linked) = is_link(host, &next) else {
            return Walk::Answer(true);
        };
        if linked {
            let Ok(target) = host.read_link(&next) else {
                return Walk::Answer(true);
            };
            let target_text = target.to_string_lossy().into_owned();
            let segments: Vec<&str> = target_text
                .split('/')
                .filter(|segment| !segment.is_empty() && *segment != ".")
                .collect();
            let last_up = segments.iter().rposition(|segment| *segment == "..");
            let first_down = segments.iter().position(|segment| *segment != "..");
            let escapes = match (last_up, first_down) {
                (Some(up), Some(down)) => up > down,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if escapes {
                return Walk::Answer(true);
            }
            let mut joined = target;
            for rest in &parts[index + 1..] {
                joined.push(rest);
            }
            return Walk::Follow(resolve_path(&folder, &joined));
        }
        folder = next;
    }
    Walk::Answer(false)
}

/// `R4` upstream: whether some folder on the way to `path` lets the user replace it.
fn walk_finds_writable_folder<H: SettingsHost>(
    host: &H,
    path: &Path,
    identity: &Identity,
    certain: &mut HashMap<PathBuf, bool>,
) -> bool {
    let mut current = resolve_path(Path::new("/"), path);
    for _ in 0..=WRITABLE_WALK_LINK_LIMIT {
        match walk_once(host, &current, identity, certain) {
            Walk::Answer(answer) => return answer,
            Walk::Follow(next) => current = next,
        }
    }
    true
}

/// `GP` upstream. `wsl_organization` is `_X(true)`: a WSL process whose kernel reports WSL. `certain` keeps the
/// folders the walk already judged, for the rest of the resolution.
pub(crate) fn user_can_write<H: SettingsHost>(
    host: &H,
    kind: PlatformKind,
    wsl_organization: bool,
    probe: &DocumentProbe<'_>,
    certain: &mut HashMap<PathBuf, bool>,
) -> bool {
    let path = probe.path();
    let euid = host.effective_user_id();
    let uid = host.real_user_id();
    if wsl_organization {
        let text = path.to_string_lossy();
        let managed = text
            .strip_prefix(WSL_MANAGED_DIRECTORY)
            .is_some_and(|rest| rest.starts_with('/'));
        if text == WSL_REGISTRY_TOOL || managed {
            return false;
        }
    }
    if euid == Some(0) || uid == Some(0) {
        return true;
    }
    let Some(euid) = euid else {
        return false;
    };
    if kind == PlatformKind::Windows {
        return false;
    }
    let linkage = linkage(host, probe);
    let identity = identity(host, euid, uid);
    let file = if probe.opened { stat_owner(host, &path) } else { None };
    let folder = if linkage == Linkage::Direct {
        folder_owner(host, probe)
    } else {
        file
    };
    if identity_allows(&identity, file, folder) != Some(false) || linkage == Linkage::Unverified {
        return true;
    }
    if write_check(host, &path) != WriteCheck::No {
        return true;
    }
    walk_finds_writable_folder(host, &path, &identity, certain)
}

#[cfg(test)]
#[path = "settings_resolve_writable_tests.rs"]
mod tests;
