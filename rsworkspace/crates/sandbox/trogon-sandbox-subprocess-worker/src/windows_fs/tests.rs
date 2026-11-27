use std::collections::BTreeMap;

use trogon_sandbox_contract::{DirectoryMarker, GlobPattern, LiteralPath};
use trogon_sandbox_windows_fence::BinShell;

use super::*;

#[derive(Default)]
struct Fake {
    dirs: BTreeMap<String, Vec<(String, Kind)>>,
    links: BTreeMap<String, String>,
}

impl Fake {
    fn tree(entries: &[(&str, &[(&str, Kind)])]) -> Self {
        Self {
            dirs: entries
                .iter()
                .map(|(dir, names)| {
                    (
                        dir.to_lowercase(),
                        names.iter().map(|(n, k)| (n.to_string(), *k)).collect(),
                    )
                })
                .collect(),
            links: BTreeMap::new(),
        }
    }
}

impl Disk for Fake {
    fn exists(&self, path: &str) -> bool {
        let path = path.to_lowercase();
        self.dirs.contains_key(&path)
            || self
                .dirs
                .values()
                .flatten()
                .any(|(name, _)| path.rsplit('\\').next() == Some(name.to_lowercase().as_str()))
    }

    fn list(&self, path: &str) -> Vec<(String, Kind)> {
        self.dirs
            .get(&path.trim_end_matches('\\').to_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    fn resolve(&self, path: &str) -> String {
        self.links
            .get(&path.to_lowercase())
            .cloned()
            .unwrap_or_else(|| path.to_string())
    }
}

type TestError = Box<dyn std::error::Error>;

fn glob(raw: &str) -> Result<FsPathEntry, TestError> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

fn literal(raw: &str) -> Result<FsPathEntry, TestError> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::Unmarked,
    })
}

fn literal_directory(raw: &str) -> Result<FsPathEntry, TestError> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::Directory,
    })
}

fn work() -> Fake {
    Fake::tree(&[
        (
            r"c:\work",
            &[("A.KEY", Kind::Other), ("b.txt", Kind::Other), ("sub", Kind::Directory)],
        ),
        (r"c:\work\sub", &[("c.key", Kind::Other)]),
    ])
}

#[test]
fn a_pattern_becomes_what_it_matches_ignoring_case() -> Result<(), Box<dyn std::error::Error>> {
    let found = expand(&[glob("/C:/work/**/*.key")?], Mode::Deny, &work())?;
    let mut found: Vec<String> = found.iter().map(|p| p.to_lowercase()).collect();
    found.sort();
    assert_eq!(found, vec![r"c:\work\a.key", r"c:\work\sub\c.key"]);
    Ok(())
}

#[test]
fn a_pattern_that_matches_nothing_adds_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let found = expand(&[glob("/C:/work/*.pem")?], Mode::Deny, &work())?;
    assert!(found.is_empty());
    Ok(())
}

#[test]
fn a_grant_that_is_not_there_is_dropped_and_a_deny_is_kept() -> Result<(), Box<dyn std::error::Error>> {
    let entries = [literal("/C:/work/sub")?, literal("/C:/missing")?];
    assert_eq!(expand(&entries, Mode::Grant, &work())?, vec![r"C:\work\sub"]);
    assert_eq!(
        expand(&entries, Mode::Deny, &work())?,
        vec![r"C:\work\sub", r"C:\missing"]
    );
    Ok(())
}

#[test]
fn a_deny_on_a_missing_directory_keeps_its_trailing_separator() -> Result<(), Box<dyn std::error::Error>> {
    let missing = literal_directory("/C:/missing/dir")?;
    let there = literal_directory("/C:/work/sub")?;
    let entries = [missing, there];
    assert_eq!(
        expand(&entries, Mode::Deny, &work())?,
        vec![r"C:\missing\dir\", r"C:\work\sub"]
    );
    assert!(expand(&entries, Mode::Grant, &work())? == vec![r"C:\work\sub"]);
    Ok(())
}

#[test]
fn a_network_path_is_never_looked_at() -> Result<(), Box<dyn std::error::Error>> {
    let entries = [literal("/UNC/srv/share/x")?];
    assert_eq!(expand(&entries, Mode::Grant, &Fake::default())?, vec![r"\\srv\share\x"]);
    Ok(())
}

#[test]
fn a_pattern_in_a_deny_reaches_the_request() -> Result<(), Box<dyn std::error::Error>> {
    let policy = FsPolicy::new(vec![glob("/C:/work/*.key")?], vec![], vec![], vec![], vec![], vec![])?;
    let mut request = ExecRequest::new("cmd", BinShell::parse(None, r"C:\Windows")?);
    apply_fs_policy_on(&mut request, &policy, &work())?;
    assert_eq!(request.deny_read, vec![r"C:\work\A.KEY"]);
    Ok(())
}

#[test]
fn a_grant_is_compared_by_the_object_it_leads_to() -> Result<(), Box<dyn std::error::Error>> {
    let mut disk = work();
    disk.links.insert(r"c:\link".to_string(), r"C:\work\sub".to_string());
    disk.dirs.insert(r"c:\link".to_string(), vec![]);
    let policy = FsPolicy::new(
        vec![literal("/C:/work")?],
        vec![],
        vec![literal("/C:/link")?],
        vec![],
        vec![],
        vec![],
    )?;
    let mut request = ExecRequest::new("cmd", BinShell::parse(None, r"C:\Windows")?);
    apply_fs_policy_on(&mut request, &policy, &disk)?;
    assert_eq!(request.deny_read_except, vec![r"C:\link"]);
    Ok(())
}
