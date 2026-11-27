type TestResult = Result<(), Box<dyn std::error::Error>>;
use super::*;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_copy_is_private_while_its_source_is_still_streaming() -> TestResult {
    let root = tempfile::tempdir()?;
    let source = root.path().join("source");
    let target = root.path().join("target");
    let name = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
    let mut writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&source)?;
    let host_umask = || -> Option<String> {
        std::fs::read_to_string("/proc/self/status")
            .ok()?
            .lines()
            .find(|line| line.starts_with("Umask:"))
            .map(str::to_owned)
    };
    let before = host_umask();
    let copied = target.clone();
    let task = tokio::spawn(async move { copy_private(&source, &copied).await });
    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Ok(stat) = tokio::fs::metadata(&target).await {
                break stat.permissions().mode() & 0o777;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    writer.write_all(b"private-data")?;
    drop(writer);
    tokio::time::timeout(std::time::Duration::from_secs(3), task).await???;
    assert_eq!(std::fs::read(&target)?, b"private-data");
    assert_eq!(host_umask(), before, "copy must not change the worker's umask");
    assert_eq!(
        observed & 0o077,
        0,
        "copy exposed its destination before the final chmod"
    );
    Ok(())
}

#[tokio::test]
async fn private_copy_does_not_modify_its_base() -> TestResult {
    let dir = tempfile::tempdir()?;
    let base = dir.path().join("base");
    let copy = dir.path().join("copy");
    tokio::fs::write(&base, b"base-data").await?;
    copy_private(&base, &copy).await?;
    tokio::fs::write(&copy, b"lease-data").await?;
    assert_eq!(tokio::fs::read(&base).await?, b"base-data");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn sparse_base_does_not_expand_to_its_logical_size() -> TestResult {
    let dir = tempfile::tempdir()?;
    let base = dir.path().join("base");
    let copy = dir.path().join("copy");
    std::fs::File::create(&base)?.set_len(64 * 1024 * 1024)?;
    copy_private(&base, &copy).await?;
    let stat = std::fs::metadata(copy)?;
    assert_eq!(stat.len(), 64 * 1024 * 1024);
    assert!(stat.blocks() * 512 < 1024 * 1024);
    Ok(())
}
