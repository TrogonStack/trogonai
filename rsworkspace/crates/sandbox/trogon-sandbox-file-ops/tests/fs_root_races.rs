use std::error::Error;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::thread;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tempfile::TempDir;
use trogon_sandbox_contract::{
    Base64Data, FileContentHashing, FileOp, FileOpErrorKind, FileOpResponse, FileParentCreation, FileRecursion,
    FileWriteTruncation,
};
use trogon_sandbox_file_ops::{FileOpError, FsRoot, SandboxPrefix};

type TestResult = Result<(), Box<dyn Error>>;

fn root() -> Result<(TempDir, FsRoot), Box<dyn Error>> {
    let dir = TempDir::new()?;
    let fs_root = FsRoot::new(dir.path(), Some(SandboxPrefix::new("/workspace")?))?;
    Ok((dir, fs_root))
}

fn write_op(path: &str, bytes: &[u8], mode: Option<u32>) -> Result<FileOp, Box<dyn Error>> {
    Ok(FileOp::Write {
        path: path.into(),
        data: Base64Data::new(STANDARD.encode(bytes))?,
        offset: 0,
        truncate: FileWriteTruncation::Truncate,
        mode,
        create_parents: FileParentCreation::Create,
    })
}

fn read_op(path: &str) -> FileOp {
    FileOp::Read {
        path: path.into(),
        offset: 0,
        length: None,
        max_bytes: None,
    }
}

fn edit_op(path: &str, find: &str, replace: &str) -> FileOp {
    FileOp::Edit {
        path: path.into(),
        find: find.into(),
        replace: replace.into(),
        expected_occurrences: 1,
    }
}

fn content_of(response: FileOpResponse) -> Result<Vec<u8>, Box<dyn Error>> {
    match response {
        FileOpResponse::Content { data, .. } => Ok(STANDARD.decode(data.as_str())?),
        other => Err(format!("expected content, got {other:?}").into()),
    }
}

#[cfg(unix)]
#[test]
fn an_edit_preserves_the_file_mode() -> TestResult {
    let (dir, fs_root) = root()?;
    fs_root.execute(&write_op("s.sh", b"one", Some(0o700))?);
    let reply = fs_root.execute(&edit_op("s.sh", "one", "two"));
    assert!(matches!(reply, FileOpResponse::Edited { .. }), "{reply:?}");
    assert_eq!(fs::read(dir.path().join("s.sh"))?, b"two");
    let mode = fs::metadata(dir.path().join("s.sh"))?.permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    Ok(())
}

#[test]
fn concurrent_edits_never_lose_an_update() -> TestResult {
    const WORKERS: u32 = 8;
    const ROUNDS: u32 = 20;
    let (_dir, fs_root) = root()?;
    fs_root.execute(&write_op("counter.txt", b"[0]", None)?);
    let fs_root = Arc::new(fs_root);
    let handles: Vec<_> = (0..WORKERS)
        .map(|_| {
            let fs_root = Arc::clone(&fs_root);
            thread::spawn(move || -> Result<(), String> {
                for _ in 0..ROUNDS {
                    loop {
                        let data = content_of(fs_root.execute(&read_op("counter.txt"))).map_err(|e| e.to_string())?;
                        let text = String::from_utf8(data).map_err(|e| e.to_string())?;
                        let current: u32 = text
                            .trim_matches(['[', ']'])
                            .parse()
                            .map_err(|e| format!("{text}: {e}"))?;
                        let reply = fs_root.execute(&edit_op(
                            "counter.txt",
                            &format!("[{current}]"),
                            &format!("[{}]", current + 1),
                        ));
                        if matches!(reply, FileOpResponse::Edited { .. }) {
                            break;
                        }
                    }
                }
                Ok(())
            })
        })
        .collect();
    for handle in handles {
        handle.join().map_err(|_| "an editing thread panicked")??;
    }
    let data = content_of(fs_root.execute(&read_op("counter.txt")))?;
    assert_eq!(String::from_utf8(data)?, format!("[{}]", WORKERS * ROUNDS));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_root_is_followed() -> TestResult {
    let (dir, fs_root) = root()?;
    fs_root.execute(&write_op("real.txt", b"data", None)?);
    std::os::unix::fs::symlink("real.txt", dir.path().join("alias"))?;
    assert_eq!(content_of(fs_root.execute(&read_op("alias")))?, b"data");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_manifest_records_a_symlink_without_walking_into_its_target() -> TestResult {
    let (dir, fs_root) = root()?;
    let outside = TempDir::new()?;
    fs::write(outside.path().join("secret.txt"), b"x")?;
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link"))?;
    fs::write(dir.path().join("a.txt"), b"1")?;

    let manifest = fs_root.manifest("/workspace", FileRecursion::Recursive, FileContentHashing::Content)?;
    let paths: Vec<_> = manifest.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths, ["/workspace/a.txt", "/workspace/link"]);
    let link = manifest.get("/workspace/link").ok_or("the link is missing")?;
    assert!(!link.is_directory());
    assert!(link.hash().is_none());
    Ok(())
}

#[test]
fn a_manifest_refuses_a_path_outside_the_root() -> TestResult {
    let (_dir, fs_root) = root()?;
    for path in ["/workspace/../etc", "/etc"] {
        let refused = fs_root.manifest(path, FileRecursion::Recursive, FileContentHashing::Metadata);
        assert!(
            matches!(
                &refused,
                Err(FileOpError::Failed {
                    kind: FileOpErrorKind::InvalidPath,
                    ..
                })
            ),
            "{path}: {refused:?}"
        );
    }
    Ok(())
}
