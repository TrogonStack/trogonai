#![cfg(target_os = "linux")]

//! The directories between a
//! protected path and the write root cannot be moved, exchanged or removed,
//! so the protection cannot be dodged by renaming the ground away.

use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, LiteralPath};
use trogon_sandbox_policy_exec::{CommandSpec, FilesystemView, NetworkPolicy, PolicyExec, PolicyExecConfig};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn can_sandbox() -> bool {
    PolicyExec::is_supported_platform() && std::process::Command::new("bwrap").arg("--version").output().is_ok()
}

fn has_python() -> bool {
    std::process::Command::new("python3").arg("--version").output().is_ok()
}

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn project() -> Result<Project, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?.join("project");
    std::fs::create_dir_all(root.join(".git/hooks"))?;
    std::fs::write(root.join(".git/config"), "[core]\n")?;
    std::fs::create_dir_all(root.join("sub"))?;
    std::fs::write(root.join("sub/file.txt"), "data\n")?;
    std::fs::create_dir_all(root.join("exch"))?;
    Ok(Project { _dir: dir, root })
}

async fn run(
    write_root: &Path,
    cwd: &Path,
    command: &str,
) -> Result<(bool, String, String), Box<dyn std::error::Error>> {
    let allow = FsPathEntry::Literal {
        path: LiteralPath::new(write_root.to_str().ok_or("utf8 path")?)?,
        directory: DirectoryMarker::Unmarked,
    };
    let policy = FsPolicy::new(vec![], vec![], vec![], vec![], vec![allow], vec![])?;
    let config = PolicyExecConfig::new(cwd)
        .view(FilesystemView::Host)
        .network(NetworkPolicy::DenyAll)
        .fs_policy(policy)
        .helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;
    let output = runtime.run(CommandSpec::shell(command)).await?;
    Ok((output.success(), output.stdout_text(), output.stderr_text()))
}

fn busy(text: &str) -> bool {
    text.to_lowercase().contains("busy")
}

const EXCHANGE: &str = "python3 -c \"import ctypes, sys; libc = ctypes.CDLL(None, use_errno=True); r = libc.renameat2(-100, b'.git', -100, b'exch', 2); print('exchange=' + ('OK' if r == 0 else str(ctypes.get_errno())))\"";

fn intact(project: &Project) -> TestResult {
    assert_eq!(std::fs::read_to_string(project.root.join(".git/config"))?, "[core]\n");
    assert!(!project.root.join(".git-moved").exists());
    Ok(())
}

#[tokio::test]
async fn renaming_the_protected_directory_aside_is_refused_as_busy() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let project = project()?;

    let (_, said, failed) = run(
        &project.root,
        &project.root,
        "echo work > newfile.txt && (mv .git .git-moved && mkdir .git && echo planted > .git/config) 2>&1; echo DONE",
    )
    .await?;

    assert!(said.contains("DONE") && busy(&said), "{said} {failed}");
    assert!(project.root.join("newfile.txt").exists());
    intact(&project)?;
    Ok(())
}

#[tokio::test]
async fn removing_the_protected_directory_is_refused_as_busy() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let project = project()?;

    let (_, said, failed) = run(&project.root, &project.root, "rmdir .git; echo DONE").await?;

    assert!(said.contains("DONE"), "{said}");
    assert!(busy(&failed), "{failed}");
    assert!(project.root.join(".git").exists());
    Ok(())
}

#[tokio::test]
async fn exchanging_the_protected_directory_is_refused_as_busy() -> TestResult {
    if !can_sandbox() || !has_python() {
        return Ok(());
    }
    let project = project()?;

    let (_, said, _) = run(&project.root, &project.root, EXCHANGE).await?;

    assert!(said.contains("exchange=16"), "{said}");
    assert!(project.root.join(".git/config").exists());
    Ok(())
}

#[tokio::test]
async fn ordinary_work_beside_and_inside_the_protected_directory_still_works() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let project = project()?;
    std::fs::write(project.root.join("README.md"), "readme\n")?;

    let (ok, said, failed) = run(
        &project.root,
        &project.root,
        "echo idx > .git/index && mkdir .git/objects && echo n > newfile.txt && mv sub sub-renamed && mv README.md .git/README.md && echo ALL_OK",
    )
    .await?;

    assert!(ok && said.contains("ALL_OK"), "{said} {failed}");
    assert!(project.root.join(".git/index").exists());
    assert!(project.root.join("sub-renamed").exists());
    assert!(project.root.join(".git/README.md").exists());
    Ok(())
}

fn root_write() -> PathBuf {
    PathBuf::from("/")
}

fn probe(steps: &[(&str, String)]) -> String {
    let mut script = String::from("import os\n");
    for (label, call) in steps {
        script.push_str(&format!(
            "try:\n    {call}\n    print('{label}=OK')\nexcept OSError as e:\n    print('{label}=' + __import__('errno').errorcode[e.errno])\n"
        ));
    }
    format!("python3 -c \"{}\"", script.replace('"', "\\\""))
}

// The pins are buried under the write root bind (and a cover under a root
// grant), so a raw rename(2) or link(2) across a pinned ancestor stays on one
// mount.
#[tokio::test]
async fn renames_and_links_work_across_every_pinned_ancestor_under_a_root_write_grant() -> TestResult {
    across_every_pinned_ancestor(|_| root_write()).await?;
    Ok(())
}

#[tokio::test]
async fn renames_and_links_work_across_every_pinned_ancestor_under_an_ordinary_write_root() -> TestResult {
    across_every_pinned_ancestor(|project| project.root.parent().map_or_else(PathBuf::new, Path::to_path_buf)).await?;
    Ok(())
}

async fn across_every_pinned_ancestor(write_root: impl Fn(&Project) -> PathBuf) -> TestResult {
    if !can_sandbox() || !has_python() {
        return Ok(());
    }
    let project = project()?;
    let git = project.root.join(".git");
    std::fs::write(git.join("b.txt"), "b\n")?;
    std::fs::write(project.root.join("a.txt"), "a\n")?;
    std::fs::write(project.root.join("sub/c.txt"), "c\n")?;
    let outside = project.root.parent().ok_or("no parent")?.join("outside.txt");
    std::fs::write(&outside, "o\n")?;
    let at = |rel: &str| format!("{:?}", project.root.join(rel).display().to_string());
    let command = probe(&[
        ("within", format!("os.rename({}, {})", at("a.txt"), at("sub/a2.txt"))),
        (
            "intoGit",
            format!("os.rename({}, {})", at("sub/c.txt"), at(".git/c2.txt")),
        ),
        ("outOfGit", format!("os.rename({}, {})", at(".git/b.txt"), at("b2.txt"))),
        (
            "intoProject",
            format!(
                "os.rename({:?}, {})",
                outside.to_str().ok_or("utf8 path")?,
                at("o2.txt")
            ),
        ),
        (
            "linkIntoGit",
            format!("os.link({}, {})", at("sub/a2.txt"), at(".git/hard.txt")),
        ),
        ("writeInGit", format!("open({}, 'w').write('x')", at(".git/new.txt"))),
    ]);

    let (_, said, failed) = run(&write_root(&project), &project.root, &command).await?;

    for label in [
        "within",
        "intoGit",
        "outOfGit",
        "intoProject",
        "linkIntoGit",
        "writeInGit",
    ] {
        assert!(said.contains(&format!("{label}=OK")), "{said} {failed}");
    }
    assert!(project.root.join(".git/hard.txt").exists());
    assert!(project.root.join("o2.txt").exists());
    Ok(())
}

#[tokio::test]
async fn moving_or_removing_a_pinned_ancestor_is_refused_under_a_root_write_grant() -> TestResult {
    if !can_sandbox() || !has_python() {
        return Ok(());
    }
    let project = project()?;
    let base = project.root.parent().ok_or("no parent")?.to_path_buf();
    let git = project.root.join(".git");
    let command = probe(&[
        (
            "moveGit",
            format!(
                "os.rename({:?}, {:?})",
                git.to_str().ok_or("utf8 path")?,
                project.root.join(".git-aside").to_str().ok_or("utf8 path")?
            ),
        ),
        (
            "moveProject",
            format!(
                "os.rename({:?}, {:?})",
                project.root.to_str().ok_or("utf8 path")?,
                base.join("project2").to_str().ok_or("utf8 path")?
            ),
        ),
        ("rmdirGit", format!("os.rmdir({:?})", git.to_str().ok_or("utf8 path")?)),
        (
            "rmdirHooks",
            format!("os.rmdir({:?})", git.join("hooks").to_str().ok_or("utf8 path")?),
        ),
    ]);

    let (_, said, failed) = run(&root_write(), &project.root, &command).await?;

    for label in ["moveGit", "moveProject", "rmdirGit", "rmdirHooks"] {
        assert!(said.contains(&format!("{label}=EBUSY")), "{said} {failed}");
    }
    intact(&project)?;
    assert!(!base.join("project2").exists());
    Ok(())
}

#[tokio::test]
async fn exchanging_the_protected_directory_is_refused_under_a_root_write_grant() -> TestResult {
    if !can_sandbox() || !has_python() {
        return Ok(());
    }
    let project = project()?;

    let (_, said, failed) = run(
        &root_write(),
        &project.root,
        &format!("cd {} && {EXCHANGE}", project.root.display()),
    )
    .await?;

    assert!(said.contains("exchange=16"), "{said} {failed}");
    assert!(project.root.join(".git/config").exists());
    Ok(())
}

#[tokio::test]
async fn renaming_the_protected_directory_aside_is_refused_under_a_root_write_grant() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let project = project()?;

    let (_, said, failed) = run(
        &root_write(),
        &project.root,
        "echo work > newfile.txt && (mv .git .git-moved && mkdir .git && echo planted > .git/config) 2>&1; echo DONE",
    )
    .await?;

    assert!(said.contains("DONE") && busy(&said), "{said} {failed}");
    assert!(project.root.join("newfile.txt").exists());
    intact(&project)?;
    Ok(())
}

#[tokio::test]
async fn moving_files_across_pinned_ancestors_works_under_a_root_write_grant() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let project = project()?;
    std::fs::write(project.root.join("a.txt"), "a\n")?;
    std::fs::write(project.root.join("sub/c.txt"), "c\n")?;
    std::fs::write(project.root.join(".git/b.txt"), "b\n")?;

    let (ok, said, failed) = run(
        &root_write(),
        &project.root,
        "mv a.txt sub/a2.txt && mv sub/c.txt .git/c2.txt && mv .git/b.txt b2.txt && echo x > .git/new.txt && echo ALL_OK",
    )
    .await?;

    assert!(ok && said.contains("ALL_OK"), "{said} {failed}");
    assert!(project.root.join("sub/a2.txt").exists());
    assert!(project.root.join(".git/c2.txt").exists());
    assert!(project.root.join("b2.txt").exists());
    assert!(project.root.join(".git/new.txt").exists());
    Ok(())
}
