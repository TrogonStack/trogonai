use std::sync::Mutex;

use trogon_sandbox_contract::FileKind;

use super::{ConfinedPath, PathVetoError, Probe, verify_no_symlinks};
use crate::lock::lock;

type Table<'a> = [(&'a str, Result<Probe, &'a str>)];

fn confined(paths: &[&str]) -> ConfinedPath {
    ConfinedPath {
        path: paths.last().map(|p| (*p).to_string()).unwrap_or_default(),
        below_root: paths.iter().map(|p| (*p).to_string()).collect(),
    }
}

async fn probe_with(confined: &ConfinedPath, table: &Table<'_>) -> (Result<(), PathVetoError>, Vec<String>) {
    let seen = Mutex::new(Vec::new());
    let result = verify_no_symlinks(confined, |path| {
        lock(&seen).push(path.clone());
        let answer = table
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, r)| r.map_err(str::to_string))
            .unwrap_or(Ok(Probe::Missing));
        async move { answer }
    })
    .await;
    (result, seen.into_inner().unwrap_or_default())
}

#[tokio::test]
async fn a_symlink_below_the_root_is_refused() {
    let confined = confined(&["/workspace/link", "/workspace/link/secret"]);
    let (result, seen) = probe_with(&confined, &[("/workspace/link", Ok(Probe::Kind(FileKind::Symlink)))]).await;
    assert_eq!(result, Err(PathVetoError::Symlink("/workspace/link".into())));
    assert_eq!(seen, vec!["/workspace/link".to_string()]);
}

#[tokio::test]
async fn plain_directories_pass_and_a_missing_path_stops_the_walk() {
    let confined = confined(&["/workspace/a", "/workspace/a/b", "/workspace/a/b/c"]);
    let (result, seen) = probe_with(&confined, &[("/workspace/a", Ok(Probe::Kind(FileKind::Directory)))]).await;
    assert_eq!(result, Ok(()));
    assert_eq!(seen, vec!["/workspace/a".to_string(), "/workspace/a/b".to_string()]);
}

#[tokio::test]
async fn a_walk_that_finds_only_existing_directories_passes() {
    let confined = confined(&["/workspace/a"]);
    let (result, _) = probe_with(&confined, &[("/workspace/a", Ok(Probe::Kind(FileKind::File)))]).await;
    assert_eq!(result, Ok(()));
}

#[tokio::test]
async fn a_failed_probe_fails_closed() {
    let confined = confined(&["/workspace/a"]);
    let (result, _) = probe_with(&confined, &[("/workspace/a", Err("unsupported"))]).await;
    assert_eq!(
        result,
        Err(PathVetoError::Unverified {
            path: "/workspace/a".into(),
            reason: "unsupported".into()
        })
    );
}
