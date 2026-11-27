mod transfer_support;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use transfer_support::{Harness, TestResult, pattern};
use trogon_sandbox_client::{ClientError, FILE_TRANSFER_CHUNK_BYTES, TransportError};
use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileOpResponse, FileResultTruncation, FileStat};

#[tokio::test]
async fn a_directory_tree_round_trips_with_nested_and_empty_entries() -> TestResult {
    let h = Harness::new()?;
    let src = h.local.path().join("src");
    fs::create_dir_all(src.join("a/b"))?;
    fs::create_dir_all(src.join("empty"))?;
    fs::write(src.join("top.txt"), b"top")?;
    fs::write(src.join("a/mid.txt"), pattern(10_000))?;
    let deep = pattern(FILE_TRANSFER_CHUNK_BYTES as usize + 5);
    fs::write(src.join("a/b/deep.bin"), &deep)?;

    let up = h.client.upload_dir(&h.lease, &src, "/tree", None).await?;
    assert_eq!(up.files, 3);
    assert_eq!(up.directories, 4);
    assert!(h.sandbox.path().join("tree/empty").is_dir());

    let dest = h.local.path().join("dest");
    let down = h.client.download_dir(&h.lease, "/tree", &dest, None).await?;
    assert_eq!(down.files, 3);
    assert_eq!(fs::read(dest.join("top.txt"))?, b"top");
    assert_eq!(fs::read(dest.join("a/b/deep.bin"))?, deep);
    assert!(dest.join("empty").is_dir());

    let again = h.client.download_dir(&h.lease, "/tree", &dest, None).await?;
    assert_eq!(again.skipped_files, 3);
    assert_eq!(again.bytes_transferred, 0);
    Ok(())
}

#[tokio::test]
async fn a_trailing_slash_on_the_remote_directory_joins_cleanly() -> TestResult {
    let h = Harness::new()?;
    let src = h.local.path().join("src");
    fs::create_dir_all(&src)?;
    fs::write(src.join("f"), b"x")?;
    h.client.upload_dir(&h.lease, &src, "/tree/", None).await?;
    assert_eq!(fs::read(h.sandbox.path().join("tree/f"))?, b"x");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_counted_and_never_copied() -> TestResult {
    let h = Harness::new()?;
    let src = h.local.path().join("src");
    fs::create_dir_all(&src)?;
    fs::write(src.join("real"), b"x")?;
    std::os::unix::fs::symlink("real", src.join("link"))?;
    let up = h.client.upload_dir(&h.lease, &src, "/tree", None).await?;
    assert_eq!(up.files, 1);
    assert_eq!(up.skipped_special, 1);
    assert!(!h.sandbox.path().join("tree/link").exists());

    fs::create_dir_all(h.sandbox.path().join("out"))?;
    fs::write(h.sandbox.path().join("out/real"), b"y")?;
    std::os::unix::fs::symlink("real", h.sandbox.path().join("out/link"))?;
    let dest = h.local.path().join("dest");
    let down = h.client.download_dir(&h.lease, "/out", &dest, None).await?;
    assert_eq!(down.files, 1);
    assert_eq!(down.skipped_special, 1);
    assert!(!dest.join("link").exists());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn file_modes_travel_in_both_directions() -> TestResult {
    let h = Harness::new()?;
    let src = h.local.path().join("src");
    fs::create_dir_all(&src)?;
    fs::write(src.join("run.sh"), b"#!/bin/sh\n")?;
    fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o750))?;
    h.client.upload_dir(&h.lease, &src, "/tree", None).await?;
    let remote_mode = fs::metadata(h.sandbox.path().join("tree/run.sh"))?.permissions().mode() & 0o7777;
    assert_eq!(remote_mode, 0o750);

    let dest = h.local.path().join("dest");
    h.client.download_dir(&h.lease, "/tree", &dest, None).await?;
    let local_mode = fs::metadata(dest.join("run.sh"))?.permissions().mode() & 0o7777;
    assert_eq!(local_mode, 0o750);
    Ok(())
}

#[tokio::test]
async fn a_source_that_is_not_a_directory_is_refused() -> TestResult {
    let h = Harness::new()?;
    let file = h.local.path().join("f");
    fs::write(&file, b"x")?;
    let error = h
        .client
        .upload_dir(&h.lease, &file, "/tree", None)
        .await
        .err()
        .ok_or("the upload should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::NotADirectory { .. })
    ));
    let error = h
        .client
        .upload_dir(&h.lease, &h.local.path().join("missing"), "/tree", None)
        .await
        .err()
        .ok_or("the upload should fail")?;
    assert!(matches!(error, ClientError::Transport(TransportError::Local { .. })));
    Ok(())
}

#[tokio::test]
async fn a_listing_cut_at_the_entry_ceiling_is_refused() -> TestResult {
    let h = Harness::tampering(|_, reply| match reply {
        FileOpResponse::Listing { entries, .. } => FileOpResponse::Listing {
            entries,
            truncated: FileResultTruncation::Truncated,
        },
        other => other,
    })?;
    fs::create_dir(h.sandbox.path().join("tree"))?;
    let error = h
        .client
        .download_dir(&h.lease, "/tree", &h.local.path().join("dest"), None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(
        error,
        ClientError::FileOpFailed {
            kind: FileOpErrorKind::TooLarge,
            ..
        }
    ));
    Ok(())
}

fn listing_with(path: &'static str) -> impl Fn(&FileOp, FileOpResponse) -> FileOpResponse + Send + Sync {
    move |_, reply| match reply {
        FileOpResponse::Listing { .. } => FileOpResponse::Listing {
            entries: vec![FileStat {
                path: path.to_string(),
                name: "x".to_string(),
                kind: trogon_sandbox_contract::FileKind::Directory,
                size: 0,
                mode: None,
                modified_unix_ms: None,
                symlink_target: None,
            }],
            truncated: FileResultTruncation::Complete,
        },
        other => other,
    }
}

#[tokio::test]
async fn a_listing_path_cannot_escape_the_destination() -> TestResult {
    for hostile in ["../etc/passwd", "/etc/passwd"] {
        let h = Harness::tampering(listing_with(hostile))?;
        fs::create_dir(h.sandbox.path().join("tree"))?;
        let dest = h.local.path().join("dest");
        let error = h
            .client
            .download_dir(&h.lease, "/tree", &dest, None)
            .await
            .err()
            .ok_or("the download should fail")?;
        assert!(matches!(
            error,
            ClientError::Transport(TransportError::PathEscapes { .. })
        ));
    }
    let h = Harness::tampering(listing_with("./a/b"))?;
    fs::create_dir(h.sandbox.path().join("tree"))?;
    let dest = h.local.path().join("dest");
    let report = h.client.download_dir(&h.lease, "/tree", &dest, None).await?;
    assert_eq!(report.directories, 1);
    assert!(dest.join("a/b").is_dir());
    Ok(())
}

#[tokio::test]
async fn a_listing_without_modes_leaves_the_local_permissions_alone() -> TestResult {
    let h = Harness::tampering(|_, reply| match reply {
        FileOpResponse::Listing { mut entries, truncated } => {
            for entry in &mut entries {
                entry.mode = None;
            }
            FileOpResponse::Listing { entries, truncated }
        }
        other => other,
    })?;
    fs::create_dir(h.sandbox.path().join("tree"))?;
    fs::write(h.sandbox.path().join("tree/f"), b"x")?;
    let dest = h.local.path().join("dest");
    let report = h.client.download_dir(&h.lease, "/tree", &dest, None).await?;
    assert_eq!(report.files, 1);
    assert_eq!(fs::read(dest.join("f"))?, b"x");
    Ok(())
}
