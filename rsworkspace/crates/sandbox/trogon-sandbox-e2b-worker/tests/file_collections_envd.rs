#![cfg(unix)]

mod support;

use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;

use serde::Deserialize;
use support::{Fixture, TestResult, decode, entry_names, outcome};
use trogon_sandbox_contract::{
    Base64Data, FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, LeaseId, SearchMatch,
};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};
use trogon_sandbox_file_ops::{ArchiveSummary, FsRoot};

#[derive(Deserialize)]
struct SearchResults {
    matches: Vec<SearchMatch>,
    files_scanned: u64,
    truncated: bool,
}

#[derive(Deserialize)]
struct Archive {
    data: Base64Data,
    entries: u64,
    bytes: u64,
}

fn request(op: FileOp) -> FileOpRequest {
    FileOpRequest {
        lease_id: LeaseId::new("lease-files"),
        as_user: None,
        op,
    }
}

fn search(path: String, pattern: Option<&str>, regex: bool, limit: Option<u32>) -> FileOp {
    FileOp::Search {
        path,
        glob: Some("*.txt".into()),
        pattern: pattern.map(str::to_string),
        regex: regex.into(),
        case_insensitive: true.into(),
        max_results: limit,
        max_file_bytes: None,
    }
}

#[tokio::test]
async fn search_preserves_recursive_globs_regex_case_and_result_limits_without_following_links() -> TestResult {
    let fixture = Fixture::new().await?;
    std::fs::create_dir_all(fixture.path("tree/nested"))?;
    std::fs::write(fixture.path("tree/a.txt"), "First NEEDLE\nsecond needle\n")?;
    std::fs::write(fixture.path("tree/nested/b.txt"), "third Needle\n")?;
    std::fs::write(fixture.path("tree/ignored.rs"), "needle\n")?;
    std::fs::write(fixture.path("outside.txt"), "outside needle\n")?;
    std::os::unix::fs::symlink(fixture.path("outside.txt"), fixture.path("tree/link.txt"))?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let path = fixture.path("tree").to_string_lossy().into_owned();
    let reply = exec
        .handle_file_op(&fixture.sandbox, &request(search(path.clone(), None, false, None)))
        .await;
    let SearchResults {
        matches,
        files_scanned,
        truncated,
    } = outcome(&reply)?;
    assert_eq!(
        matches.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(),
        ["a.txt", "nested/b.txt"]
    );
    assert!(matches.iter().all(|m| m.line.is_none() && m.text.is_none()));
    assert_eq!(files_scanned, 0);
    assert!(!truncated);

    std::os::unix::fs::symlink(fixture.path("tree"), fixture.path("alias"))?;
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(search(
                fixture.path("alias").to_string_lossy().into_owned(),
                None,
                false,
                None,
            )),
        )
        .await;
    let SearchResults { matches, .. } = outcome(&reply)?;
    assert_eq!(
        matches.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(),
        ["a.txt", "nested/b.txt"]
    );

    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(search(path.clone(), Some("needle$"), false, None)),
        )
        .await;
    assert!(matches!(reply, FileOpResponse::SearchResults { matches, .. } if matches.is_empty()));
    let mut small_files = search(path.clone(), Some("needle"), false, None);
    if let FileOp::Search { max_file_bytes, .. } = &mut small_files {
        *max_file_bytes = Some(2);
    }
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &request(small_files)).await,
        FileOpResponse::SearchResults {
            matches: vec![],
            files_scanned: 0,
            truncated: false.into()
        }
    );

    let reply = exec
        .handle_file_op(&fixture.sandbox, &request(search(path, Some("needle$"), true, Some(2))))
        .await;
    let SearchResults { matches, truncated, .. } = outcome(&reply)?;
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].line, Some(1));
    assert_eq!(matches[0].text.as_deref(), Some("First NEEDLE"));
    assert!(truncated);
    Ok(())
}

#[tokio::test]
async fn invalid_search_patterns_are_refused_before_guest_calls() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(search(
                fixture.path("tree").to_string_lossy().into_owned(),
                Some("["),
                true,
                None,
            )),
        )
        .await;
    assert!(
        matches!(
            reply,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::InvalidRequest,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(fixture.calls(), 0);
    Ok(())
}

#[tokio::test]
async fn archives_roundtrip_plain_trees_and_preserve_binary_content() -> TestResult {
    let fixture = Fixture::new().await?;
    std::fs::create_dir_all(fixture.path("tree/nested"))?;
    std::fs::write(fixture.path("tree/one"), [0, 255, 1])?;
    std::fs::write(fixture.path("tree/nested/two"), b"second")?;
    std::os::unix::fs::symlink("one", fixture.path("tree/link"))?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::ArchiveGet {
                path: fixture.path("tree").to_string_lossy().into_owned(),
                max_bytes: None,
            }),
        )
        .await;
    let Archive { data, entries, bytes } = outcome(&reply)?;
    assert_eq!((entries, bytes), (3, 9));
    let summary = ArchiveSummary::validate(&decode(&data)?)?;
    assert_eq!((summary.entries(), summary.bytes()), (3, 9));
    assert_eq!(
        exec.handle_file_op(
            &fixture.sandbox,
            &request(FileOp::ArchivePut {
                path: fixture.path("restored").to_string_lossy().into_owned(),
                data,
            })
        )
        .await,
        FileOpResponse::Extracted { entries: 3, bytes: 9 }
    );
    assert_eq!(std::fs::read(fixture.path("restored/one"))?, [0, 255, 1]);
    assert_eq!(std::fs::read(fixture.path("restored/nested/two"))?, b"second");
    assert!(!fixture.path("restored/link").exists());
    assert!(
        !entry_names(&fixture.path(""))?
            .iter()
            .any(|name| name.contains("trogon-archive"))
    );
    Ok(())
}

fn archive_entry(name: &str, kind: u8, link: &str) -> Vec<u8> {
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    header[124..136].copy_from_slice(b"00000000000\0");
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].fill(b' ');
    header[156] = kind;
    header[157..157 + link.len()].copy_from_slice(link.as_bytes());
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u32 = header.iter().map(|b| u32::from(*b)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    let mut bytes = header.to_vec();
    bytes.resize(1536, 0);
    bytes
}

#[tokio::test]
async fn archive_validation_refuses_traversal_links_and_devices_before_any_guest_mutation() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    for entry in [
        archive_entry("../escape", b'0', ""),
        archive_entry("link", b'2', "../escape"),
        archive_entry("node", b'3', ""),
    ] {
        let data = support::data(&entry)?;
        let reply = exec
            .handle_file_op(
                &fixture.sandbox,
                &request(FileOp::ArchivePut {
                    path: fixture.path("dest").to_string_lossy().into_owned(),
                    data,
                }),
            )
            .await;
        assert!(
            matches!(
                reply,
                FileOpResponse::Failed {
                    kind: FileOpErrorKind::InvalidRequest,
                    ..
                }
            ),
            "{reply:?}"
        );
    }
    assert_eq!(fixture.calls(), 0);
    assert!(!fixture.path("dest").exists());
    Ok(())
}

#[tokio::test]
async fn archive_extraction_refuses_existing_destination_symlink_components() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    std::fs::create_dir_all(fixture.path("source/link"))?;
    std::fs::write(fixture.path("source/link/keep"), b"replacement")?;
    std::fs::create_dir(fixture.path("outside"))?;
    std::fs::write(fixture.path("outside/keep"), b"preserve")?;
    std::fs::create_dir(fixture.path("dest"))?;
    std::os::unix::fs::symlink(fixture.path("outside"), fixture.path("dest/link"))?;
    let source = FsRoot::new(fixture.path("source"), None)?;
    let Archive { data, .. } = outcome(&source.execute(&FileOp::ArchiveGet {
        path: "/".into(),
        max_bytes: None,
    }))?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::ArchivePut {
                path: fixture.path("dest").to_string_lossy().into_owned(),
                data,
            }),
        )
        .await;
    assert!(
        matches!(
            reply,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::InvalidPath,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(std::fs::read(fixture.path("outside/keep"))?, b"preserve");
    assert!(std::fs::symlink_metadata(fixture.path("dest/link"))?.is_symlink());
    Ok(())
}

#[tokio::test]
async fn archive_capture_preserves_a_file_larger_than_the_read_chunk_limit() -> TestResult {
    let fixture = Fixture::new().await?;
    let size = FileOp::MAX_READ_BYTES + 1;
    let mut source = std::fs::File::create(fixture.path("large"))?;
    source.set_len(size)?;
    source.seek(SeekFrom::End(-1))?;
    source.write_all(&[255])?;
    drop(source);
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::ArchiveGet {
                path: fixture.path("large").to_string_lossy().into_owned(),
                max_bytes: None,
            }),
        )
        .await;
    let Archive { data, entries, bytes } = outcome(&reply)?;
    assert_eq!((entries, bytes), (1, size));
    let target = tempfile::TempDir::new()?;
    assert_eq!(
        FsRoot::new(target.path(), None)?.execute(&FileOp::ArchivePut { path: "/".into(), data }),
        FileOpResponse::Extracted {
            entries: 1,
            bytes: size
        }
    );
    let content = std::fs::read(target.path().join("large"))?;
    assert_eq!(content.len() as u64, size);
    assert_eq!(content.last(), Some(&255));
    Ok(())
}

#[tokio::test]
async fn archive_capture_refuses_a_tree_over_the_requested_content_limit() -> TestResult {
    let fixture = Fixture::new().await?;
    std::fs::create_dir(fixture.path("tree"))?;
    std::fs::write(fixture.path("tree/data"), b"too large")?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::ArchiveGet {
                path: fixture.path("tree").to_string_lossy().into_owned(),
                max_bytes: Some(2),
            }),
        )
        .await;
    assert!(
        matches!(
            reply,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::TooLarge,
                ..
            }
        ),
        "{reply:?}"
    );
    Ok(())
}
