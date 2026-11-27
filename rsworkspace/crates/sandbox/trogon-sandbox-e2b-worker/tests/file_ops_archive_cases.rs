#![cfg(unix)]

mod file_ops_guest_support;

use file_ops_guest_support::{Guest, TestResult, directory, file, no_content, ok, payload, status};
use serde_json::json;
use trogon_sandbox_contract::{
    FileCaseSensitivity, FileOp, FileOpErrorKind, FileOpResponse, FileResultTruncation, FileSearchSyntax,
};

type Failure = Box<dyn std::error::Error>;

fn tar_entry(
    builder: &mut tar::Builder<Vec<u8>>,
    name: &str,
    content: &[u8],
    is_directory: bool,
) -> Result<(), Failure> {
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(if is_directory {
        tar::EntryType::Directory
    } else {
        tar::EntryType::Regular
    });
    header.set_cksum();
    builder.append_data(&mut header, name, content)?;
    Ok(())
}

fn archive_put(path: &str, tar: &[u8]) -> Result<FileOp, Failure> {
    Ok(FileOp::ArchivePut {
        path: path.into(),
        data: payload(tar)?,
    })
}

fn single_file_tar() -> Result<Vec<u8>, Failure> {
    let mut builder = tar::Builder::new(Vec::new());
    tar_entry(&mut builder, "a.txt", b"hi", false)?;
    Ok(builder.into_inner()?)
}

fn failed(reply: &FileOpResponse, expected: FileOpErrorKind, fragment: &str) -> bool {
    matches!(reply, FileOpResponse::Failed { kind, message } if *kind == expected && message.contains(fragment))
}

fn search(path: &str, glob: &str, max_file_bytes: Option<u64>) -> FileOp {
    FileOp::Search {
        path: path.into(),
        glob: Some(glob.into()),
        pattern: Some("needle".into()),
        regex: FileSearchSyntax::Literal,
        case_insensitive: FileCaseSensitivity::Insensitive,
        max_results: None,
        max_file_bytes,
    }
}

#[tokio::test]
async fn archive_upload_refuses_unusable_destinations() -> TestResult {
    let tar = single_file_tar()?;
    let guest = Guest::new().await?;
    let reply = guest.run(archive_put("", &tar)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::InvalidPath, "nonempty path"),
        "{reply:?}"
    );

    guest.stat("/w/file", file("/w/file", 1)).await;
    let reply = guest.run(archive_put("/w/file", &tar)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::NotADirectory, "not a directory"),
        "{reply:?}"
    );

    guest.stat_reply("/w/broken", status(500)).await;
    let reply = guest.run(archive_put("/w/broken", &tar)?).await;
    assert!(failed(&reply, FileOpErrorKind::Io, "unavailable"), "{reply:?}");

    guest.stat("/w/dest", directory("/w/dest")).await;
    guest.stat_reply("/w/dest/a.txt", status(500)).await;
    let reply = guest.run(archive_put("/w/dest", &tar)?).await;
    assert!(failed(&reply, FileOpErrorKind::Io, "unavailable"), "{reply:?}");
    Ok(())
}

#[tokio::test]
async fn archive_upload_reports_extraction_and_cleanup_failures() -> TestResult {
    let tar = single_file_tar()?;
    let extraction = Guest::new().await?;
    extraction.upload(no_content()).await;
    extraction.remove(ok()).await;
    extraction.script(1, "tar: short read").await?;
    let reply = extraction.run(archive_put("/w/dest", &tar)?).await;
    assert!(
        failed(
            &reply,
            FileOpErrorKind::Io,
            "archive extraction failed (exit 1): tar: short read"
        ),
        "{reply:?}"
    );

    let cleanup = Guest::new().await?;
    cleanup.upload(no_content()).await;
    cleanup.remove(status(500)).await;
    cleanup.script(0, "").await?;
    let reply = cleanup.run(archive_put("/w/dest", &tar)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::Io, "staging cleanup failed"),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn archive_upload_skips_the_root_entry() -> TestResult {
    let mut builder = tar::Builder::new(Vec::new());
    tar_entry(&mut builder, "./", b"", true)?;
    tar_entry(&mut builder, "a.txt", b"hi", false)?;
    let guest = Guest::new().await?;
    guest.upload(no_content()).await;
    guest.remove(ok()).await;
    guest.script(0, "").await?;
    let reply = guest.run(archive_put("/w/dest", &builder.into_inner()?)?).await;
    assert_eq!(reply, FileOpResponse::Extracted { entries: 1, bytes: 2 });
    Ok(())
}

#[tokio::test]
async fn archive_upload_bounds_entries_destination_paths_and_stream_size() -> TestResult {
    let guest = Guest::new().await?;
    let mut many = tar::Builder::new(Vec::new());
    for index in 0..=FileOp::MAX_ENTRIES {
        tar_entry(&mut many, &format!("f{index}"), b"", false)?;
    }
    let reply = guest.run(archive_put("/w/dest", &many.into_inner()?)?).await;
    assert!(failed(&reply, FileOpErrorKind::TooLarge, "entry limit"), "{reply:?}");

    let mut nested = tar::Builder::new(Vec::new());
    for index in 0..=FileOp::MAX_ENTRIES / 2 {
        tar_entry(&mut nested, &format!("d{index}/f"), b"", false)?;
    }
    let reply = guest.run(archive_put("/w/dest", &nested.into_inner()?)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::TooLarge, "too many destination components"),
        "{reply:?}"
    );

    // A stream that fills the limit exactly has no room for the closing blocks of the canonical copy.
    let content = usize::try_from(FileOp::MAX_ARCHIVE_BYTES)? - 512;
    let mut header = tar::Header::new_gnu();
    header.set_path("big.bin")?;
    header.set_size(content as u64);
    header.set_mode(0o644);
    header.set_cksum();
    let mut full = header.as_bytes().to_vec();
    full.resize(512 + content, 0);
    let reply = guest.run(archive_put("/w/dest", &full)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::TooLarge, "canonical tar stream"),
        "{reply:?}"
    );
    assert_eq!(guest.requests().await, 0);
    Ok(())
}

#[tokio::test]
async fn archive_download_stops_at_the_entry_limit_and_at_changed_files() -> TestResult {
    let guest = Guest::new().await?;
    let entries: Vec<_> = (0..=FileOp::MAX_ENTRIES)
        .map(|index| file(&format!("/w/many/f{index}"), 0))
        .collect();
    guest.stat("/w/many", directory("/w/many")).await;
    guest.list_dir("/w/many", json!({"entries": entries})).await;
    let reply = guest
        .run(FileOp::ArchiveGet {
            path: "/w/many".into(),
            max_bytes: None,
        })
        .await;
    assert!(failed(&reply, FileOpErrorKind::TooLarge, "entry limit"), "{reply:?}");

    guest.stat("/w/short", file("/w/short", 5)).await;
    guest.download("/w/short", b"abc").await;
    let reply = guest
        .run(FileOp::ArchiveGet {
            path: "/w/short".into(),
            max_bytes: None,
        })
        .await;
    assert!(
        failed(&reply, FileOpErrorKind::Io, "file changed during collection"),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn search_matches_path_globs_against_the_relative_path() -> TestResult {
    let guest = Guest::new().await?;
    guest.stat("/w/tree", directory("/w/tree")).await;
    guest
        .list_dir(
            "/w/tree",
            json!({"entries": [directory("/w/tree/sub"), file("/w/tree/top.txt", 6)]}),
        )
        .await;
    guest
        .list_dir("/w/tree/sub", json!({"entries": [file("/w/tree/sub/a.txt", 6)]}))
        .await;
    guest.download("/w/tree/sub/a.txt", b"Needle").await;
    let reply = guest.run(search("/w/tree", "sub/*.txt", None)).await;
    assert!(
        matches!(&reply, FileOpResponse::SearchResults { matches, files_scanned: 1, .. } if matches.len() == 1),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn search_reports_a_cut_short_scan_when_the_staging_budget_runs_out() -> TestResult {
    let guest = Guest::new().await?;
    guest.stat("/w/huge", directory("/w/huge")).await;
    guest
        .list_dir("/w/huge", json!({"entries": [file("/w/huge/a.txt", 300_000_000)]}))
        .await;
    let reply = guest.run(search("/w/huge", "*.txt", Some(400_000_000))).await;
    assert!(
        matches!(
            &reply,
            FileOpResponse::SearchResults {
                truncated: FileResultTruncation::Truncated,
                files_scanned: 0,
                ..
            }
        ),
        "{reply:?}"
    );
    Ok(())
}
