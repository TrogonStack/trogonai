use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use super::*;
use crate::bridge_refs::guess_default_branch;

const GATED_GOLDENS: &str = include_str!("../tests/fixtures/bridge_create_session/gated.json");

fn os(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}

fn lookup<'a>(pairs: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsString> {
    pairs.iter().find(|(name, _)| name == key).map(|(_, value)| value)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn replay_setup(root: &Path, case: &Value) -> io::Result<()> {
    let marker = root.to_string_lossy().into_owned();
    for step in case["setup"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if step.get("git").is_some() {
            let cwd = root.join(step["cwd"].as_str().unwrap_or("."));
            let status = Command::new("git")
                .args(strings(&step["git"]))
                .current_dir(cwd)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()?
                .status;
            assert!(status.success(), "{step}");
        } else if let Some(dir) = step["mkdir"].as_str() {
            fs::create_dir_all(root.join(dir))?;
        } else if step.get("write").is_some() {
            let parts = strings(&step["write"]);
            fs::write(root.join(&parts[0]), parts[1].replace("<root>", &marker))?;
        } else if let Some(file) = step["remove"].as_str() {
            fs::remove_file(root.join(file))?;
        } else if step.get("symlink").is_some() {
            let parts = strings(&step["symlink"]);
            #[cfg(unix)]
            std::os::unix::fs::symlink(parts[1].replace("<root>", &marker), root.join(&parts[0]))?;
            #[cfg(not(unix))]
            let _ = parts;
        }
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn guesses_gated_repositories_like_the_upstream_bridge() -> io::Result<()> {
    let cases: Vec<Value> = serde_json::from_str(GATED_GOLDENS).unwrap_or_default();
    assert!(cases.len() >= 38);
    for case in &cases {
        let name = case["name"].as_str().unwrap_or_default();
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        replay_setup(&root, case)?;
        let mut context = GitProbeContext::from_process();
        context.session_cwd = case["session"].as_str().map(|session| root.join(session));
        context.blocked = case["blocked"].as_bool().unwrap_or_default();
        let process = root.join(case["process"].as_str().unwrap_or("main"));
        let expected = case["guess"].as_str().unwrap_or_default();
        assert_eq!(guess_default_branch(&context, &process).await, expected, "{name}");
    }
    Ok(())
}

#[test]
fn scrubs_the_environment_the_way_the_upstream_runner_does() {
    let base = os(&[
        ("HOME", "/home/a"),
        ("GIT_DIR", "/elsewhere"),
        ("git_work_tree", "/elsewhere"),
        ("GIT_CONFIG_KEY_0", "k"),
        ("git_config_value_12", "v"),
        ("GIT_CONFIG_KEY_X", "kept"),
        ("LC_ALL", "en_US"),
        ("Language", "de"),
        ("PATH", "rel:/usr/bin::/bin"),
    ]);
    let result = sanitised_environment(&base, Path::new("/r/.git"), Path::new("/r/.git"));
    assert_eq!(lookup(&result, "HOME"), Some(&OsString::from("/home/a")));
    assert_eq!(lookup(&result, "GIT_DIR"), Some(&OsString::from("/r/.git")));
    assert_eq!(lookup(&result, "GIT_COMMON_DIR"), Some(&OsString::from("/r/.git")));
    assert_eq!(lookup(&result, "GIT_IMPLICIT_WORK_TREE"), Some(&OsString::from("0")));
    assert_eq!(lookup(&result, "git_work_tree"), None);
    assert_eq!(lookup(&result, "GIT_CONFIG_KEY_0"), None);
    assert_eq!(lookup(&result, "git_config_value_12"), None);
    assert_eq!(lookup(&result, "GIT_CONFIG_KEY_X"), Some(&OsString::from("kept")));
    assert_eq!(lookup(&result, "LC_ALL"), Some(&OsString::from("C")));
    assert_eq!(lookup(&result, "Language"), None);
    assert_eq!(lookup(&result, "LANGUAGE"), Some(&OsString::from("C")));
    assert_eq!(lookup(&result, "PATH"), Some(&OsString::from("/usr/bin:/bin")));
    assert_eq!(lookup(&result, "GIT_TERMINAL_PROMPT"), Some(&OsString::from("0")));
    assert_eq!(lookup(&result, "GIT_ALLOW_PROTOCOL"), Some(&OsString::from("/")));
    assert_eq!(lookup(&result, "GIT_SSH_COMMAND"), Some(&OsString::from("false")));
    assert_eq!(
        lookup(&result, "GIT_CONFIG_PARAMETERS"),
        Some(&OsString::from(CONFIG_PARAMETERS.join(" ")))
    );
}

#[test]
fn cleans_search_paths_down_to_absolute_entries() {
    assert_eq!(clean_search_path("/a:/b"), "/a:/b");
    assert_eq!(clean_search_path("a:b::"), "/dev/null");
    assert_eq!(clean_search_path(""), "/dev/null");
    assert_eq!(clean_search_path("x:/y"), "/y");
    assert!(config_slot("GIT_CONFIG_VALUE_007"));
    assert!(!config_slot("GIT_CONFIG_KEY_"));
    assert!(!config_slot("GIT_CONFIG_COUNT"));
    assert!(!config_slot("OTHER_KEY_1"));
    assert!(unusable_inode(0));
    assert!(unusable_inode(u64::MAX));
    assert!(!unusable_inode(7));
}

#[test]
fn keeps_the_default_context_open_to_the_real_process() {
    let context = GitProbeContext::default();
    assert!(!context.blocked);
    assert_eq!(context.timeout, Duration::from_secs(5));
    assert_eq!(context.session_cwd, std::env::current_dir().ok());
    assert!(context.environment.iter().any(|(key, _)| key == "PATH") || cfg!(not(unix)));
    let moved = context.with_session_cwd("/elsewhere");
    assert_eq!(moved.session_cwd.as_deref(), Some(Path::new("/elsewhere")));
}

#[cfg(unix)]
fn script(root: &Path, name: &str, body: &str) -> io::Result<PathBuf> {
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

#[cfg(unix)]
fn plain_repository(root: &Path) -> io::Result<GitProbeContext> {
    fs::create_dir_all(root.join("work/.git"))?;
    let mut context = GitProbeContext::from_process();
    context.session_cwd = Some(root.join("work"));
    Ok(context)
}

#[cfg(unix)]
#[tokio::test]
async fn runs_the_program_with_the_scrubbed_environment() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let mut context = plain_repository(&root)?;
    context.git_program = script(&root, "fake", "printf '%s|%s|%s' \"$GIT_DIR\" \"$LC_ALL\" \"$1\"")?;
    let output = context.run(&root.join("work/.git"), &["x"]).await;
    let output = output.map(|output| (output.success, output.stdout));
    let git_dir = root.join("work/.git");
    assert_eq!(output, Some((true, format!("{}|C|-c", git_dir.display()))));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_to_run_outside_the_gate() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let mut context = plain_repository(&root)?;
    context.git_program = script(&root, "fake", "printf ran")?;
    let repository = root.join("work/.git");
    context.blocked = true;
    assert!(context.run(&repository, &[]).await.is_none());
    context.blocked = false;
    assert!(context.run(&repository, &[]).await.is_some());
    context.session_cwd = None;
    assert!(context.run(&repository, &[]).await.is_none());
    context.session_cwd = Some(root.join("missing"));
    assert!(context.run(&repository, &[]).await.is_none());
    context.session_cwd = Some(root.join("work"));
    assert!(context.run(&root.join("missing/.git"), &[]).await.is_none());
    fs::create_dir_all(root.join("other/.git"))?;
    context.session_cwd = Some(root.join("other"));
    assert!(context.run(&repository, &[]).await.is_none());
    let file = root.join("work/.git/not-a-directory");
    fs::write(&file, "x")?;
    context.session_cwd = Some(root.join("work"));
    assert!(context.run(&file, &[]).await.is_none());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_a_git_directory_that_is_not_a_worktree_of_the_common_one() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let mut context = plain_repository(&root)?;
    context.git_program = script(&root, "fake", "printf ran")?;
    fs::create_dir_all(root.join("b/.git"))?;
    fs::create_dir_all(root.join("a/.git"))?;
    fs::write(
        root.join("a/.git/commondir"),
        format!("{}\n", root.join("b/.git").display()),
    )?;
    assert!(context.run(&root.join("a/.git"), &[]).await.is_none());
    fs::create_dir_all(root.join("b/.git/worktrees/w"))?;
    fs::write(
        root.join("b/.git/worktrees/w/commondir"),
        format!("{}\n", root.join("b/.git").display()),
    )?;
    context.session_cwd = Some(root.join("b"));
    assert!(context.run(&root.join("b/.git/worktrees/w"), &[]).await.is_some());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn gives_up_on_a_child_that_outlives_the_limit_or_cannot_start() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let mut context = plain_repository(&root)?;
    context.git_program = script(&root, "slow", "sleep 30")?;
    context.timeout = Duration::from_millis(100);
    let repository = root.join("work/.git");
    let slow = context.run(&repository, &[]).await;
    assert_eq!(
        slow.map(|output| (output.success, output.stdout)),
        Some((false, String::new()))
    );
    context.git_program = root.join("missing-program");
    let missing = context.run(&repository, &[]).await;
    assert_eq!(missing.map(|output| output.success), Some(false));
    context.git_program = script(&root, "fails", "printf partial; exit 3")?;
    let failed = context.run(&repository, &[]).await;
    assert_eq!(
        failed.map(|output| (output.success, output.stdout)),
        Some((false, "partial".to_owned()))
    );
    Ok(())
}
