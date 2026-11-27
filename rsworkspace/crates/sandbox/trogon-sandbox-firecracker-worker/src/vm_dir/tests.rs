use super::*;

fn state(path: &Path) -> StateDir {
    StateDir::new(path)
}

#[test]
fn a_lease_id_cannot_escape_the_state_dir() -> Result<(), Box<dyn std::error::Error>> {
    let state = state(Path::new("/var/lib/fc"));
    for id in ["", "..", "a/b", "a b", "../x", "a\0b"] {
        assert!(
            VmDir::for_lease(&state, &LeaseId::new(id.to_string())).is_err(),
            "{id:?}"
        );
    }
    let dir = VmDir::for_lease(&state, &LeaseId::new("01ARZ3NDEKTSV4RRFFQ69G5FAV"))?;
    assert_eq!(
        dir.api_socket(),
        Path::new("/var/lib/fc/01ARZ3NDEKTSV4RRFFQ69G5FAV/api.sock")
    );
    assert_eq!(
        dir.vsock_socket(),
        Path::new("/var/lib/fc/01ARZ3NDEKTSV4RRFFQ69G5FAV/v.sock")
    );
    Ok(())
}

#[test]
fn an_overlong_state_dir_is_refused_up_front() -> Result<(), Box<dyn std::error::Error>> {
    let long = format!("/{}", "x".repeat(120));
    let err = VmDir::for_lease(&state(Path::new(&long)), &LeaseId::new("l1"))
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(err, VmDirError::SocketPathTooLong { .. }));
    Ok(())
}

#[tokio::test]
async fn remove_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dir = VmDir::for_lease(&state(tmp.path()), &LeaseId::new("l1"))?;
    dir.create().await?;
    tokio::fs::write(dir.rootfs(), b"x").await?;
    dir.create().await?;

    dir.remove().await?;
    assert!(!dir.as_path().exists());
    dir.remove().await?;
    Ok(())
}

#[tokio::test]
async fn stale_dirs_lists_only_directories() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    tokio::fs::create_dir(tmp.path().join("a")).await?;
    tokio::fs::write(tmp.path().join("note"), b"x").await?;
    let found = stale_dirs(&state(tmp.path())).await?;
    assert_eq!(found.len(), 1);
    assert!(found[0].as_path().ends_with("a"));

    let missing = stale_dirs(&state(&tmp.path().join("nope"))).await?;
    assert!(missing.is_empty());
    Ok(())
}
