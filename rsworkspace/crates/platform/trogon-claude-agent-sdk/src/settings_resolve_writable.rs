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
    connector_vetting::Located,
    constants::{
        MODE_GROUP_WRITABLE, MODE_OTHER_WRITABLE, WRITABLE_WALK_LINK_LIMIT, WSL_MANAGED_DIRECTORY, WSL_REGISTRY_TOOL,
    },
    settings_resolve_connector::locate_document,
    settings_resolve_platform_kind::PlatformKind,
    settings_resolve_sources::resolve_path,
    settings_resolve_state::ResolverState,
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

/// `E4` upstream.
fn linkage<H: SettingsHost>(host: &H, state: &ResolverState, probe: &DocumentProbe<'_>) -> Linkage {
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
    let (document, folder) = locate_document(host, state, probe.root, &probe.components);
    if document == Located::Gone || folder == Located::Gone {
        return Linkage::Direct;
    }
    if matches!(folder, Located::Unverified | Located::Network(_)) || document == Located::Unverified {
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

/// What `GP` upstream settles before it looks at owners.
enum ByIds {
    Settled(bool),
    /// The answer depends on the owners, asked for these effective and real user ids.
    AskOwners(u32, Option<u32>),
}

fn settled_by_ids<H: SettingsHost>(host: &H, kind: PlatformKind, wsl_organization: bool, path: &Path) -> ByIds {
    let euid = host.effective_user_id();
    let uid = host.real_user_id();
    if wsl_organization {
        let text = path.to_string_lossy();
        let managed = text
            .strip_prefix(WSL_MANAGED_DIRECTORY)
            .is_some_and(|rest| rest.starts_with('/'));
        if text == WSL_REGISTRY_TOOL || managed {
            return ByIds::Settled(false);
        }
    }
    if euid == Some(0) || uid == Some(0) {
        return ByIds::Settled(true);
    }
    let Some(euid) = euid else {
        return ByIds::Settled(false);
    };
    if kind == PlatformKind::Windows {
        return ByIds::Settled(false);
    }
    ByIds::AskOwners(euid, uid)
}

/// The rest of `GP` upstream, once the owners of the document and of the folder it counts are known.
fn owners_or_folders_allow<H: SettingsHost>(
    host: &H,
    path: &Path,
    identity: &Identity,
    file: Option<Owner>,
    folder: Option<Owner>,
    unverified: bool,
    certain: &mut HashMap<PathBuf, bool>,
) -> bool {
    if identity_allows(identity, file, folder) != Some(false) || unverified {
        return true;
    }
    if write_check(host, path) != WriteCheck::No {
        return true;
    }
    walk_finds_writable_folder(host, path, identity, certain)
}

/// `GP` upstream. `wsl_organization` is `_X(true)`: a WSL process whose kernel reports WSL. `certain` keeps the
/// folders the walk already judged, for the rest of the resolution.
pub(crate) fn user_can_write<H: SettingsHost>(
    host: &H,
    state: &ResolverState,
    kind: PlatformKind,
    wsl_organization: bool,
    probe: &DocumentProbe<'_>,
    certain: &mut HashMap<PathBuf, bool>,
) -> bool {
    let path = probe.path();
    let (euid, uid) = match settled_by_ids(host, kind, wsl_organization, &path) {
        ByIds::Settled(answer) => return answer,
        ByIds::AskOwners(euid, uid) => (euid, uid),
    };
    let linkage = linkage(host, state, probe);
    let identity = identity(host, euid, uid);
    let file = if probe.opened { stat_owner(host, &path) } else { None };
    let folder = if linkage == Linkage::Direct {
        folder_owner(host, probe)
    } else {
        file
    };
    owners_or_folders_allow(
        host,
        &path,
        &identity,
        file,
        folder,
        linkage == Linkage::Unverified,
        certain,
    )
}

/// `GP(new Bs, Mi, ...)` upstream: whether the user could replace the registry tool WSL runs. Nothing has noted a
/// link for it yet, so the owners are those of the tool and of its folder as `stat` reports them.
pub(crate) fn registry_tool_writable<H: SettingsHost>(host: &H, kind: PlatformKind, wsl_organization: bool) -> bool {
    let path = Path::new(WSL_REGISTRY_TOOL);
    let (euid, uid) = match settled_by_ids(host, kind, wsl_organization, path) {
        ByIds::Settled(answer) => return answer,
        ByIds::AskOwners(euid, uid) => (euid, uid),
    };
    let identity = identity(host, euid, uid);
    let file = stat_owner(host, path);
    let folder = path.parent().and_then(|folder| stat_owner(host, folder));
    owners_or_folders_allow(host, path, &identity, file, folder, false, &mut HashMap::new())
}

#[cfg(test)]
#[path = "settings_resolve_writable_tests.rs"]
mod tests;
