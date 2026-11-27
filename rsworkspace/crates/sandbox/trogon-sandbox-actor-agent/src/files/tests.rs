use std::error::Error;
use std::num::NonZeroU16;
use std::os::unix::fs::PermissionsExt;

use tempfile::TempDir;
use trogon_sandbox_contract::{LeaseId, SandboxUserName};

use super::*;

fn binding(working_dir: Option<&Path>) -> LeaseBinding {
    LeaseBinding {
        lease_id: LeaseId::new("lease"),
        env: Default::default(),
        working_dir: working_dir.map(|path| path.to_string_lossy().to_string()),
        exposed_port: NonZeroU16::new(8080),
    }
}

fn write(
    path: impl Into<String>,
    content: &[u8],
    mode: Option<u32>,
    as_user: Option<SandboxUserName>,
) -> WriteFilesRequest {
    WriteFilesRequest {
        lease_id: LeaseId::new("lease"),
        files: vec![SandboxFileWrite {
            path: path.into(),
            content: content.to_vec(),
            mode,
        }],
        as_user,
    }
}

fn read(path: impl Into<String>, as_user: Option<SandboxUserName>) -> ReadFileRequest {
    ReadFileRequest {
        lease_id: LeaseId::new("lease"),
        path: path.into(),
        as_user,
    }
}

/// The agent's own user is the one identity a non-root test process can become, so it is what exercises the
/// drop-privileges path end to end.
fn current_user() -> Result<SandboxUserName, Box<dyn Error>> {
    let current =
        nix::unistd::User::from_uid(nix::unistd::Uid::effective())?.ok_or("the running uid has no passwd entry")?;
    Ok(SandboxUserName::new(current.name)?)
}

fn scratch() -> Result<(TempDir, LeaseBinding), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let bound = binding(Some(dir.path()));
    Ok((dir, bound))
}

#[tokio::test]
async fn write_then_read_round_trips() -> Result<(), Box<dyn Error>> {
    let (_dir, bound) = scratch()?;
    assert_eq!(
        write_files(&write("nested/hello.txt", b"hi", None, None), &bound).await,
        WriteFilesResponse::Written
    );
    assert_eq!(
        read_file(&read("nested/hello.txt", None), &bound).await,
        ReadFileResponse::Content { data: b"hi".to_vec() }
    );
    Ok(())
}

#[tokio::test]
async fn mode_is_applied_when_requested() -> Result<(), Box<dyn Error>> {
    let (dir, bound) = scratch()?;
    assert_eq!(
        write_files(&write("script.sh", b"#!/bin/sh\n", Some(0o755), None), &bound).await,
        WriteFilesResponse::Written
    );
    let metadata = std::fs::metadata(dir.path().join("script.sh"))?;
    assert_eq!(metadata.permissions().mode() & 0o777, 0o755);
    Ok(())
}

#[tokio::test]
async fn absolute_paths_ignore_the_working_directory() -> Result<(), Box<dyn Error>> {
    let (dir, bound) = scratch()?;
    let elsewhere = tempfile::tempdir()?;
    let target = elsewhere.path().join("absolute.txt");
    let request = write(target.to_string_lossy(), b"x", None, None);
    assert_eq!(write_files(&request, &bound).await, WriteFilesResponse::Written);
    assert!(target.exists());
    assert!(!dir.path().join("absolute.txt").exists());
    Ok(())
}

#[tokio::test]
async fn without_a_working_directory_a_relative_path_stays_relative() -> Result<(), Box<dyn Error>> {
    assert_eq!(resolve("a/b", None), PathBuf::from("a/b"));
    let response = read_file(&read("definitely/not/here.txt", None), &binding(None)).await;
    assert_eq!(response, ReadFileResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_path_without_a_parent_component_is_attempted_as_is() -> Result<(), Box<dyn Error>> {
    let response = write_files(&write(".", b"x", None, None), &binding(None)).await;
    assert!(
        matches!(response, WriteFilesResponse::Denied(LeaseError::Backend { .. })),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn reading_a_missing_file_is_not_found() -> Result<(), Box<dyn Error>> {
    let (_dir, bound) = scratch()?;
    assert_eq!(
        read_file(&read("missing.txt", None), &bound).await,
        ReadFileResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn reading_a_directory_is_denied() -> Result<(), Box<dyn Error>> {
    let (dir, bound) = scratch()?;
    std::fs::create_dir(dir.path().join("sub"))?;
    let response = read_file(&read("sub", None), &bound).await;
    assert!(
        matches!(response, ReadFileResponse::Denied(LeaseError::Backend { .. })),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn writing_under_a_file_is_denied() -> Result<(), Box<dyn Error>> {
    let (dir, bound) = scratch()?;
    std::fs::write(dir.path().join("plain"), b"x")?;
    let response = write_files(&write("plain/child.txt", b"x", None, None), &bound).await;
    assert!(
        matches!(response, WriteFilesResponse::Denied(LeaseError::Backend { .. })),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn writing_over_a_directory_is_denied() -> Result<(), Box<dyn Error>> {
    let (dir, bound) = scratch()?;
    std::fs::create_dir(dir.path().join("sub"))?;
    let response = write_files(&write("sub", b"x", None, None), &bound).await;
    assert!(
        matches!(response, WriteFilesResponse::Denied(LeaseError::Backend { .. })),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_unknown_user_is_an_invalid_spec() -> Result<(), Box<dyn Error>> {
    let (_dir, bound) = scratch()?;
    let user = SandboxUserName::new("definitely-not-a-user-xyz")?;
    let response = write_files(&write("x", b"", None, Some(user.clone())), &bound).await;
    assert!(matches!(
        response,
        WriteFilesResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    let response = read_file(&read("x", Some(user)), &bound).await;
    assert!(matches!(
        response,
        ReadFileResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn as_user_writes_and_reads_through_that_users_own_process() -> Result<(), Box<dyn Error>> {
    let user = current_user()?;
    let (dir, bound) = scratch()?;
    let target = dir.path().join("nested/as-user.txt");
    let content = b"line one\n'quoted'\n";
    let request = write(target.to_string_lossy(), content, Some(0o600), Some(user.clone()));
    assert_eq!(write_files(&request, &bound).await, WriteFilesResponse::Written);
    assert_eq!(std::fs::metadata(&target)?.permissions().mode() & 0o777, 0o600);

    let response = read_file(&read(target.to_string_lossy(), Some(user)), &bound).await;
    assert_eq!(response, ReadFileResponse::Content { data: content.to_vec() });
    Ok(())
}

#[tokio::test]
async fn as_user_without_a_mode_leaves_the_default_permissions() -> Result<(), Box<dyn Error>> {
    let user = current_user()?;
    let (dir, bound) = scratch()?;
    let target = dir.path().join("plain.txt");
    let request = write(target.to_string_lossy(), b"x", None, Some(user));
    assert_eq!(write_files(&request, &bound).await, WriteFilesResponse::Written);
    assert!(target.exists());
    Ok(())
}

#[tokio::test]
async fn as_user_resolves_relative_paths_against_their_home() -> Result<(), Box<dyn Error>> {
    let user = current_user()?;
    let resolved = TargetUser::resolve(&user)?;
    let (_dir, bound) = scratch()?;
    assert_eq!(base_dir(Some(&resolved), &bound).as_deref(), Some(resolved.home()));
    Ok(())
}

#[tokio::test]
async fn as_user_reading_a_missing_file_is_not_found() -> Result<(), Box<dyn Error>> {
    let user = current_user()?;
    let (dir, bound) = scratch()?;
    let missing = dir.path().join("missing.txt");
    assert_eq!(
        read_file(&read(missing.to_string_lossy(), Some(user)), &bound).await,
        ReadFileResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn as_user_reading_a_directory_is_denied() -> Result<(), Box<dyn Error>> {
    let user = current_user()?;
    let (dir, bound) = scratch()?;
    let response = read_file(&read(dir.path().to_string_lossy(), Some(user)), &bound).await;
    assert!(
        matches!(response, ReadFileResponse::Denied(LeaseError::Backend { .. })),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn as_user_writing_where_the_shell_fails_is_denied() -> Result<(), Box<dyn Error>> {
    let user = current_user()?;
    let (dir, bound) = scratch()?;
    std::fs::write(dir.path().join("plain"), b"x")?;
    let target = dir.path().join("plain/child.txt");
    let response = write_files(&write(target.to_string_lossy(), b"x", None, Some(user)), &bound).await;
    assert!(
        matches!(response, WriteFilesResponse::Denied(LeaseError::Backend { .. })),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_shell_that_cannot_start_is_a_denial_for_both_directions() -> Result<(), Box<dyn Error>> {
    let user = TargetUser::resolve(&current_user()?)?;
    let path = Path::new("/definitely/not/here");
    let written = write_as_user("/definitely/not/a/shell", path, b"x", None, &user).await;
    assert!(matches!(written, Err(LeaseError::Backend { .. })));
    let read = read_as_user("/definitely/not/a/shell", path, &user).await;
    assert!(matches!(read, ReadFileResponse::Denied(LeaseError::Backend { .. })));
    Ok(())
}
