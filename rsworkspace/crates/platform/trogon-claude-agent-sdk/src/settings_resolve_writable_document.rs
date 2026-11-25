//! The write check for a managed settings document the resolver read (`GP` upstream, with `E4`'s link handling).
//! It sits above the plain owner and folder checks because it asks the resolver state where a linked document
//! really lives.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    adapters::SettingsHost,
    connector_vetting::Located,
    settings_resolve_connector::locate_document,
    settings_resolve_platform_kind::PlatformKind,
    settings_resolve_sources::resolve_path,
    settings_resolve_state::ResolverState,
    settings_resolve_writable::{ByIds, Owner, identity, is_link, owners_or_folders_allow, settled_by_ids, stat_owner},
};

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

/// How a document sits among symbolic links (`linkOf` upstream), as far as the write check tells the cases apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Linkage {
    /// No link on the way (`null`).
    Direct,
    /// A link on the way whose ends were found (`inside` or `outside`).
    Linked,
    /// A link on the way whose ends could not be examined (`unverified`).
    Unverified,
}

/// `E4` upstream.
pub(crate) fn linkage<H: SettingsHost>(host: &H, state: &ResolverState, probe: &DocumentProbe<'_>) -> Linkage {
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

#[cfg(all(test, unix))]
#[path = "settings_resolve_writable_document_tests.rs"]
mod tests;
