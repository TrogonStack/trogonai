use crate::fs_access_lists::FsAccessLists;
use crate::fs_access_set::FsAccessSet;
use crate::policy_path::{is_beneath, key};

fn at_or_under(path: &str, dir: &str) -> bool {
    path == dir || is_beneath(path, dir)
}

fn covering<'a>(dirs: &'a [String], resolve: &dyn Fn(&str) -> String) -> impl Fn(&str) -> Option<&'a String> {
    let keys: Vec<String> = dirs.iter().map(|dir| key(&resolve(dir))).collect();
    move |path| {
        if keys.is_empty() {
            return None;
        }
        let path = key(&resolve(path));
        keys.iter()
            .position(|dir| at_or_under(&path, dir))
            .map(|index| &dirs[index])
    }
}

fn unique(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    items.into_iter().filter(|item| seen.insert(item.clone())).collect()
}

/// Ranks the access lists against each other. A grant at or beneath a credential deny is
/// left out; a write grant at or beneath a write deny becomes a read grant.
/// `resolve` gives the object a path leads to, so a link or an 8.3 name is
/// compared as what it is.
pub fn rank_fs_access(lists: &FsAccessLists, resolve: &dyn Fn(&str) -> String) -> FsAccessSet {
    let credential_over = covering(&lists.credential_deny, resolve);
    let write_deny_over = covering(&lists.deny_write, resolve);
    let mut grant_read: Vec<String> = Vec::new();
    let mut grant_write: Vec<String> = Vec::new();
    for path in &lists.allow_read {
        if credential_over(path).is_none() {
            grant_read.push(path.clone());
        }
    }
    for path in &lists.allow_write {
        let credential = credential_over(path);
        let write_deny = write_deny_over(path);
        if credential.is_none() && write_deny.is_none() {
            grant_write.push(path.clone());
        } else if credential.is_none() {
            grant_read.push(path.clone());
        }
    }
    let held_read = lists.held.as_ref().map(|held| held.grant_read.as_slice());
    let held_write = lists.held.as_ref().map(|held| held.grant_write.as_slice());
    let held_all = held_read.into_iter().flatten().chain(held_write.into_iter().flatten());
    FsAccessSet {
        grant_read: unique(grant_read),
        grant_write: unique(grant_write),
        deny_read: unique(
            lists
                .deny_read
                .iter()
                .chain(&lists.credential_deny)
                .cloned()
                .chain(held_all.filter(|path| credential_over(path).is_some()).cloned()),
        ),
        deny_write: unique(
            lists.deny_write.iter().cloned().chain(
                held_write
                    .into_iter()
                    .flatten()
                    .filter(|path| write_deny_over(path).is_some())
                    .cloned(),
            ),
        ),
    }
}
