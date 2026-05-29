//! Integration tests for `create_worktree` and `TempWorktree`.
//!
//! These tests require `git` to be available on `PATH` (standard on CI and dev boxes).
//!
//! Run with:
//!   cargo test -p trogon-runner-tools --test worktree_tests

use std::path::Path;
use std::time::Duration;
use trogon_runner_tools::worktree::create_worktree;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a throwaway temp directory that is NOT a git repository.
fn non_git_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

/// Initialise a bare-minimum git repo in a fresh temp directory and return it.
///
/// We create an initial commit so that `git worktree add HEAD` has a real HEAD
/// to check out from.
async fn git_repo_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path();

    // git init
    let status = tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(path)
        .status()
        .await
        .expect("git init should run");
    assert!(status.success(), "git init failed");

    // Configure minimal user identity so git commit works without global config.
    for (key, value) in [("user.email", "test@example.com"), ("user.name", "Test")] {
        let s = tokio::process::Command::new("git")
            .args(["config", key, value])
            .current_dir(path)
            .status()
            .await
            .expect("git config should run");
        assert!(s.success(), "git config {key} failed");
    }

    // Create an initial commit so HEAD is valid.
    let readme = path.join("README");
    std::fs::write(&readme, "init").expect("write README");

    let s = tokio::process::Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .status()
        .await
        .expect("git add should run");
    assert!(s.success(), "git add failed");

    let s = tokio::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(path)
        .status()
        .await
        .expect("git commit should run");
    assert!(s.success(), "git commit failed");

    dir
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `create_worktree` returns `None` when the directory is not a git repository.
#[tokio::test]
async fn create_worktree_returns_none_for_non_git_dir() {
    let dir = non_git_dir();
    let result = create_worktree(dir.path().to_str().unwrap()).await;
    assert!(
        result.is_none(),
        "expected None for non-git dir, got Some(path)"
    );
}

/// `create_worktree` returns `Some` for a valid git repository and the path
/// exists on disk.
#[tokio::test]
async fn create_worktree_returns_some_for_git_repo() {
    let repo = git_repo_dir().await;
    let repo_path = repo.path().to_str().unwrap();

    let worktree = create_worktree(repo_path).await;
    assert!(worktree.is_some(), "expected Some for a valid git repo");

    let wt = worktree.unwrap();
    assert!(
        Path::new(&wt.path).exists(),
        "worktree path {:?} should exist on disk",
        wt.path
    );

    // Explicit cleanup — avoids a drop-spawned background thread racing with
    // the repo tempdir being deleted.
    wt.cleanup().await;
}

/// Calling `cleanup()` removes the worktree directory.
#[tokio::test]
async fn temp_worktree_cleanup_removes_directory() {
    let repo = git_repo_dir().await;
    let repo_path = repo.path().to_str().unwrap();

    let wt = create_worktree(repo_path)
        .await
        .expect("should create worktree in valid repo");

    let wt_path = wt.path.clone();
    assert!(Path::new(&wt_path).exists(), "path should exist before cleanup");

    wt.cleanup().await;

    assert!(
        !Path::new(&wt_path).exists(),
        "path {:?} should be removed after cleanup()",
        wt_path
    );
}

/// Dropping a `TempWorktree` without calling `cleanup()` removes the directory
/// via the `Drop` impl (which spawns a background OS thread).
///
/// We sleep 500 ms after the drop to give the spawned thread time to finish.
#[tokio::test]
async fn temp_worktree_drop_removes_directory() {
    let repo = git_repo_dir().await;
    let repo_path = repo.path().to_str().unwrap();

    let wt = create_worktree(repo_path)
        .await
        .expect("should create worktree in valid repo");

    let wt_path = wt.path.clone();
    assert!(Path::new(&wt_path).exists(), "path should exist before drop");

    // Drop WITHOUT calling cleanup() — the Drop impl spawns a thread.
    drop(wt);

    // Give the background thread time to run the git worktree remove command.
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(
        !Path::new(&wt_path).exists(),
        "path {:?} should be removed after drop",
        wt_path
    );
}
