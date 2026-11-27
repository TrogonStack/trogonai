#![cfg(target_os = "linux")]

//! A read carve-out that is a symlink is restored at its own name from its vetted target, and serves none
//! of the bytes a read deny hides around that target.

use std::path::PathBuf;

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, LiteralPath};
use trogon_sandbox_policy_exec::{CommandSpec, FilesystemView, NetworkPolicy, PolicyExec, PolicyExecConfig};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn can_sandbox() -> bool {
    PolicyExec::is_supported_platform() && std::process::Command::new("bwrap").arg("--version").output().is_ok()
}

struct Tree {
    _dir: tempfile::TempDir,
    home: PathBuf,
}

fn tree() -> Result<Tree, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let home = dir.path().canonicalize()?.join("home");
    let store = home.join("store");
    std::fs::create_dir_all(store.join("pages/sec"))?;
    std::fs::create_dir_all(home.join("plain/sec"))?;
    std::fs::write(store.join("netrc"), "SECRETTOKEN\n")?;
    std::fs::write(store.join("pages/readme.md"), "PAGETEXT\n")?;
    std::fs::write(store.join("pages/sec/x"), "SECRETPAGE\n")?;
    std::fs::write(home.join("other.txt"), "HIDDEN\n")?;
    std::fs::write(home.join("plain/readme.md"), "PAGETEXT\n")?;
    std::fs::write(home.join("plain/sec/x"), "SECRETPAGE\n")?;
    std::os::unix::fs::symlink("store/netrc", home.join(".netrc"))?;
    std::os::unix::fs::symlink("store/pages", home.join("docs"))?;
    Ok(Tree { _dir: dir, home })
}

async fn run(deny: &[PathBuf], allow: &[PathBuf], command: &str) -> Result<String, Box<dyn std::error::Error>> {
    let entries = |paths: &[PathBuf]| -> Result<Vec<FsPathEntry>, Box<dyn std::error::Error>> {
        paths
            .iter()
            .map(|path| {
                Ok(FsPathEntry::Literal {
                    path: LiteralPath::new(path.to_str().ok_or("utf8 path")?)?,
                    directory: DirectoryMarker::Unmarked,
                })
            })
            .collect()
    };
    let policy = FsPolicy::new(entries(deny)?, vec![], entries(allow)?, vec![], vec![], vec![])?;
    let config = PolicyExecConfig::new(std::env::temp_dir())
        .view(FilesystemView::Host)
        .network(NetworkPolicy::DenyAll)
        .fs_policy(policy)
        .helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;
    let output = runtime.run(CommandSpec::shell(command)).await?;
    Ok(format!("{}{}", output.stdout_text(), output.stderr_text()))
}

#[tokio::test]
async fn a_symlinked_carve_out_is_restored_from_its_vetted_target() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let home = &tree.home;
    let command = format!(
        "cat {n} 2>&1; cat {d}/readme.md 2>&1; cat {o} 2>&1; echo DONE",
        n = home.join(".netrc").display(),
        d = home.join("docs").display(),
        o = home.join("other.txt").display(),
    );

    let text = run(
        std::slice::from_ref(home),
        &[home.join(".netrc"), home.join("docs")],
        &command,
    )
    .await?;

    assert!(text.contains("DONE"), "{text}");
    assert!(text.contains("SECRETTOKEN"), "{text}");
    assert!(text.contains("PAGETEXT"), "{text}");
    assert!(!text.contains("HIDDEN"), "{text}");
    Ok(())
}

#[tokio::test]
async fn a_symlinked_carve_out_serves_none_of_the_bytes_a_deny_hides() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tree = tree()?;
    let home = &tree.home;
    let store = home.join("store");

    let denied_target = run(
        &[home.clone(), store.join("netrc")],
        &[home.join(".netrc")],
        &format!("cat {} 2>&1; echo DONE", home.join(".netrc").display()),
    )
    .await?;
    assert!(denied_target.contains("DONE"), "{denied_target}");
    assert!(!denied_target.contains("SECRETTOKEN"), "{denied_target}");

    let denied_inside = run(
        &[home.clone(), store.join("pages/sec")],
        &[home.join("docs")],
        &format!("cat {}/sec/x 2>&1; echo DONE", home.join("docs").display()),
    )
    .await?;
    assert!(denied_inside.contains("DONE"), "{denied_inside}");
    assert!(!denied_inside.contains("SECRETPAGE"), "{denied_inside}");

    let denied_around = run(
        &[home.clone(), store],
        &[home.join(".netrc")],
        &format!("cat {} 2>&1; echo DONE", home.join(".netrc").display()),
    )
    .await?;
    assert!(denied_around.contains("DONE"), "{denied_around}");
    assert!(!denied_around.contains("SECRETTOKEN"), "{denied_around}");

    let plain = home.join("plain");
    let control = run(
        &[home.clone(), plain.join("sec")],
        std::slice::from_ref(&plain),
        &format!(
            "cat {p}/sec/x 2>&1; cat {p}/readme.md 2>&1; echo DONE",
            p = plain.display()
        ),
    )
    .await?;
    assert!(control.contains("DONE"), "{control}");
    assert!(!control.contains("SECRETPAGE"), "{control}");
    assert!(control.contains("PAGETEXT"), "{control}");
    Ok(())
}
