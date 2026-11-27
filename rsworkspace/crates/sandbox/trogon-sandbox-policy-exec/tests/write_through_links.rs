#![cfg(any(target_os = "linux", target_os = "macos"))]

//! A write is judged by where it lands, so a link inside the write root that
//! leads out of it gives a command nothing, and a link outside every write
//! root that leads into one is a way in like any other name.

type TestResult = Result<(), Box<dyn std::error::Error>>;

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use trogon_sandbox_contract::DirectoryMarker;
use trogon_sandbox_policy_exec::{
    CommandSpec, FilesystemView, FsPathEntry, FsPolicy, LiteralPath, NetworkPolicy, PolicyExec, PolicyExecConfig,
};

fn can_sandbox() -> bool {
    if !PolicyExec::is_supported_platform() {
        return false;
    }
    cfg!(not(target_os = "linux")) || std::process::Command::new("bwrap").arg("--version").output().is_ok()
}

struct Tree {
    _dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

fn tree() -> Result<Tree, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let base = dir.path().canonicalize()?;
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(root.join("inner"))?;
    std::fs::create_dir_all(outside.join("dir"))?;
    std::fs::write(root.join("inner/file"), "original")?;
    std::fs::write(outside.join("file"), "original")?;
    std::fs::write(outside.join("dir/file"), "original")?;
    Ok(Tree {
        _dir: dir,
        root,
        outside,
    })
}

fn attempt(command: &str) -> String {
    format!("echo started; if ( {command} ) 2>/dev/null; then echo WROTE; else echo refused; fi")
}

async fn run(tree: &Tree, command: String) -> Result<String, Box<dyn std::error::Error>> {
    let root = tree.root.to_str().ok_or("the root is not valid UTF-8")?;
    let policy = FsPolicy::new(
        vec![],
        vec![],
        vec![],
        vec![],
        vec![FsPathEntry::Literal {
            path: LiteralPath::new(root)?,
            directory: DirectoryMarker::Unmarked,
        }],
        vec![],
    )?;
    let config = PolicyExecConfig::new(&tree.outside)
        .view(FilesystemView::Host)
        .network(NetworkPolicy::DenyAll)
        .fs_policy(policy);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;
    let output = runtime.run(CommandSpec::shell(command)).await?;
    Ok(output.stdout_text())
}

fn read(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(std::fs::read_to_string(path)?)
}

#[tokio::test]
async fn a_write_through_a_link_in_the_write_root_to_a_file_outside_it_is_refused() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let link = tree.root.join("link-to-file");
    symlink(tree.outside.join("file"), &link)?;

    let said = run(&tree, attempt(&format!("echo changed > {}", link.display()))).await?;

    assert!(said.contains("started") && said.contains("refused"), "{said}");
    assert_eq!(read(&tree.outside.join("file"))?, "original");
    Ok(())
}

#[tokio::test]
async fn a_write_and_a_create_beneath_a_link_to_a_directory_outside_are_refused() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let link = tree.root.join("link-to-dir");
    symlink(tree.outside.join("dir"), &link)?;

    let written = run(&tree, attempt(&format!("echo changed > {}/file", link.display()))).await?;
    assert!(written.contains("started") && written.contains("refused"), "{written}");
    assert_eq!(read(&tree.outside.join("dir/file"))?, "original");

    let created = run(&tree, attempt(&format!("echo new > {}/new", link.display()))).await?;
    assert!(created.contains("started") && created.contains("refused"), "{created}");
    assert!(!tree.outside.join("dir/new").exists());
    Ok(())
}

#[tokio::test]
async fn a_rename_onto_a_file_outside_the_write_root_through_a_linked_directory_is_refused() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let link = tree.root.join("link-to-dir");
    symlink(tree.outside.join("dir"), &link)?;

    let said = run(
        &tree,
        attempt(&format!(
            "echo changed > {root}/made && mv {root}/made {link}/file",
            root = tree.root.display(),
            link = link.display()
        )),
    )
    .await?;

    assert!(said.contains("started") && said.contains("refused"), "{said}");
    assert_eq!(read(&tree.outside.join("dir/file"))?, "original");
    Ok(())
}

#[tokio::test]
async fn a_create_through_a_dangling_link_to_a_name_outside_is_refused() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let link = tree.root.join("dangling");
    symlink(tree.outside.join("not-there"), &link)?;

    let said = run(&tree, attempt(&format!("echo new > {}", link.display()))).await?;

    assert!(said.contains("started") && said.contains("refused"), "{said}");
    assert!(!tree.outside.join("not-there").exists());
    Ok(())
}

#[tokio::test]
async fn a_link_the_command_makes_itself_gives_nothing() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let link = tree.root.join("made-inside");

    let said = run(
        &tree,
        format!(
            "ln -s {target} {link} && {attempt}",
            target = tree.outside.join("file").display(),
            link = link.display(),
            attempt = attempt(&format!("echo changed > {}", link.display()))
        ),
    )
    .await?;

    assert!(said.contains("started") && said.contains("refused"), "{said}");
    assert_eq!(read(&tree.outside.join("file"))?, "original");
    Ok(())
}

#[tokio::test]
async fn a_write_through_a_link_that_stays_in_the_write_root_is_allowed() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let link = tree.root.join("link-inside");
    symlink(tree.root.join("inner/file"), &link)?;

    let said = run(&tree, attempt(&format!("echo changed > {}", link.display()))).await?;

    assert!(said.contains("WROTE"), "{said}");
    assert_eq!(read(&tree.root.join("inner/file"))?, "changed\n");
    Ok(())
}

#[tokio::test]
async fn a_write_through_a_link_outside_every_write_root_that_leads_into_one_is_allowed() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let link = tree.outside.join("link-into-root");
    symlink(tree.root.join("inner/file"), &link)?;

    let said = run(&tree, attempt(&format!("echo changed > {}", link.display()))).await?;

    assert!(said.contains("WROTE"), "{said}");
    assert_eq!(read(&tree.root.join("inner/file"))?, "changed\n");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_second_name_in_the_write_root_for_a_file_that_also_has_one_outside_is_writable() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let name = tree.root.join("second-name");
    std::fs::hard_link(tree.outside.join("file"), &name)?;

    let said = run(&tree, attempt(&format!("echo changed > {}", name.display()))).await?;

    assert!(said.contains("WROTE"), "{said}");
    assert_eq!(read(&tree.outside.join("file"))?, "changed\n");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_command_cannot_give_a_file_outside_the_write_root_a_second_name_in_it() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let name = tree.root.join("second-name");

    let said = run(
        &tree,
        attempt(&format!(
            "ln {} {}",
            tree.outside.join("file").display(),
            name.display()
        )),
    )
    .await?;

    assert!(said.contains("started") && said.contains("refused"), "{said}");
    assert!(!name.exists());
    Ok(())
}
