use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;

use super::*;
use crate::bridge_gitdir::read_limited;

const GUESS_GOLDENS: &str = include_str!("../tests/fixtures/bridge_create_session/guesses.json");

fn write(root: &Path, relative: &str, content: &str) -> io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}

fn strings(value: &Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, text)| (key.clone(), text.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(unix)]
#[tokio::test]
async fn guesses_the_default_branch_like_the_upstream_bridge() -> io::Result<()> {
    let context = GitProbeContext::default();
    let cases: Vec<Value> = serde_json::from_str(GUESS_GOLDENS).unwrap_or_default();
    assert!(cases.len() > 60);
    for case in &cases {
        let name = case["name"].as_str().unwrap_or_default();
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let marker = root.to_string_lossy().into_owned();
        let dirs: Vec<String> = case["dirs"].as_array().map_or_else(
            || vec!["work".to_owned()],
            |dirs| {
                dirs.iter()
                    .map(|dir| dir.as_str().unwrap_or_default().to_owned())
                    .collect()
            },
        );
        for dir in &dirs {
            fs::create_dir_all(root.join(dir))?;
        }
        for (path, content) in strings(&case["files"]) {
            write(&root, &path, &content.replace("<root>", &marker))?;
        }
        for (path, target) in strings(&case["links"]) {
            let link = root.join(&path);
            if let Some(parent) = link.parent() {
                fs::create_dir_all(parent)?;
            }
            std::os::unix::fs::symlink(target.replace("<root>", &marker), link)?;
        }
        let cwd = case["cwd"].as_str().unwrap_or("work");
        let expected = case["guess"].as_str().unwrap_or_default();
        assert_eq!(
            guess_default_branch(&context, &root.join(cwd)).await,
            expected,
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn validates_reference_names_and_object_ids() {
    for name in ["main", "a/b", "release/1.0", "a_b+c@d-e"] {
        assert!(valid_ref_name(name), "{name}");
    }
    for name in ["", "-x", "/x", "a..b", "a//b", "a/./b", "a b", "a/", "a:b"] {
        assert!(!valid_ref_name(name), "{name}");
    }
    assert!(is_object_id(&"a".repeat(40)));
    assert!(is_object_id(&"0".repeat(64)));
    assert!(!is_object_id(&"a".repeat(41)));
    assert!(!is_object_id(&"A".repeat(40)));
}

#[test]
fn reads_only_small_regular_files() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write(root, "exact", &"x".repeat(65536))?;
    write(root, "over", &"x".repeat(65537))?;
    fs::create_dir(root.join("folder"))?;
    assert_eq!(read_limited(&root.join("exact")).map(|text| text.len()), Some(65536));
    assert_eq!(read_limited(&root.join("over")), None);
    assert_eq!(read_limited(&root.join("folder")), None);
    assert_eq!(read_limited(&root.join("missing")), None);
    assert_eq!(read_text(&root.join("missing")), None);
    Ok(())
}

#[tokio::test]
async fn recognizes_packed_references() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let sha = "a".repeat(40);
    write(
        temp.path(),
        "packed-refs",
        &format!("# pack-refs\n{sha} refs/remotes/origin/main\n^{sha}\n"),
    )?;
    let packed = temp.path().join("packed-refs");
    assert!(packed_ref(&packed, "refs/remotes/origin/main"));
    assert!(!packed_ref(&packed, "refs/remotes/origin/mai"));
    assert!(!packed_ref(&temp.path().join("missing"), "refs/remotes/origin/main"));
    assert!(!resolves(&GitProbeContext::default(), temp.path(), "refs/remotes/origin/main", 0).await);
    Ok(())
}

#[tokio::test]
async fn stops_following_references_past_the_depth_limit() -> io::Result<()> {
    let context = GitProbeContext::default();
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let sha = "a".repeat(40);
    write(root, "refs/remotes/origin/r0", &format!("{sha}\n"))?;
    for step in 1..=6 {
        write(
            root,
            &format!("refs/remotes/origin/r{step}"),
            &format!("ref: refs/remotes/origin/r{}\n", step - 1),
        )?;
    }
    assert!(resolves(&context, root, "refs/remotes/origin/r4", REF_DEPTH).await);
    assert!(!resolves(&context, root, "refs/remotes/origin/r6", REF_DEPTH).await);
    Ok(())
}

#[cfg(unix)]
#[test]
fn refuses_a_directory_that_loops_through_links() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    std::os::unix::fs::symlink(root.join("b"), root.join("a"))?;
    std::os::unix::fs::symlink(root.join("a"), root.join("b"))?;
    let guard = PathGuard::host();
    let looped = root.join("a").join("x");
    assert!(!guard.walks(as_str(&looped), as_str(&root)));
    assert_eq!(find_git_dir(&guard, &looped), None);
    Ok(())
}
