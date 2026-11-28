use std::collections::VecDeque;
use std::error::Error;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use trogon_sandbox_contract::{CommandTimeoutStatus, FileWatchEventKind, OutputTruncation};
use trogon_sandbox_file_ops::Tracking;

use super::*;
use crate::support_tests::TestResult;

fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone)]
struct Call {
    script: String,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
}

struct Fake {
    download: Mutex<Option<Download>>,
    replies: Mutex<VecDeque<CommandOutput>>,
    calls: Mutex<Vec<Call>>,
    downloads: Mutex<Vec<String>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            download: Mutex::new(None),
            replies: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
            downloads: Mutex::new(Vec::new()),
        }
    }

    fn serving(self, download: Download) -> Self {
        *held(&self.download) = Some(download);
        self
    }

    fn replying(self, exit_code: i32, stdout: &str, stderr: &str) -> Self {
        held(&self.replies).push_back(CommandOutput {
            exit_code,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            timed_out: CommandTimeoutStatus::default(),
            truncated: OutputTruncation::Complete,
        });
        self
    }

    fn calls(&self) -> Vec<Call> {
        held(&self.calls).clone()
    }

    fn downloads(&self) -> Vec<String> {
        held(&self.downloads).clone()
    }
}

impl ContainerAccess for Fake {
    async fn download(&self, path: &str, _cap: u64) -> Download {
        held(&self.downloads).push(path.to_string());
        held(&self.download).take().unwrap_or(Download::Missing)
    }

    async fn sh(&self, script: &str, args: &[&str], stdin: Option<Vec<u8>>) -> anyhow::Result<CommandOutput> {
        held(&self.calls).push(Call {
            script: script.to_string(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            stdin,
        });
        held(&self.replies)
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("an unexpected exec"))
    }
}

/// Builds a tar the way the Docker archive endpoint does: entries are named from the
/// basename of the requested path.
fn tar_of(entries: &[(&str, Option<&[u8]>)]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, content) in entries {
        let mut header = tar::Header::new_gnu();
        match content {
            Some(bytes) => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(bytes.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, name, *bytes)?;
            }
            None => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, name, &[][..])?;
            }
        }
    }
    Ok(builder.into_inner()?)
}

fn b64(bytes: &[u8]) -> Result<Base64Data, Box<dyn Error>> {
    Ok(Base64Data::new(STANDARD.encode(bytes))?)
}

fn is_failed(reply: &FileOpResponse, expected: FileOpErrorKind) -> bool {
    matches!(reply, FileOpResponse::Failed { kind, .. } if *kind == expected)
}

fn source<C: ContainerAccess>(container: &C) -> ContainerSource<'_, C> {
    ContainerSource {
        container,
        inotify_missing: None,
        feeds: None,
    }
}

async fn run_op(fake: &Fake, op: &FileOp) -> FileOpResponse {
    execute(fake, op, false).await
}

fn list_op(path: &str, recursive: FileRecursion) -> FileOp {
    FileOp::List {
        path: path.into(),
        recursive,
        max_entries: None,
    }
}

fn read_op(path: &str, offset: u64, length: Option<u64>, max_bytes: Option<u64>) -> FileOp {
    FileOp::Read {
        path: path.into(),
        offset,
        length,
        max_bytes,
    }
}

fn changes_op(path: Option<&str>) -> FileOp {
    FileOp::Changes {
        since: None,
        path: path.map(str::to_string),
    }
}

#[test]
fn paths_are_normalised_and_cannot_climb_above_the_root() -> TestResult {
    assert_eq!(absolute("/a/./b//c/../d").map_err(|e| format!("{e:?}"))?, "/a/b/d");
    assert_eq!(absolute("/").map_err(|e| format!("{e:?}"))?, "/");
    assert!(matches!(
        absolute("/a/../../etc").as_ref().map_err(Refusal::response),
        Err(FileOpResponse::Failed {
            kind: FileOpErrorKind::InvalidPath,
            ..
        })
    ));
    assert!(matches!(
        absolute("relative/path").as_ref().map_err(Refusal::response),
        Err(FileOpResponse::Failed {
            kind: FileOpErrorKind::InvalidPath,
            ..
        })
    ));
    Ok(())
}

#[tokio::test]
async fn running_as_another_user_is_unsupported_and_runs_nothing() {
    let fake = Fake::new();
    let reply = execute(&fake, &FileOp::Stat { path: "/a".into() }, true).await;
    assert!(matches!(reply, FileOpResponse::Unsupported { .. }));
    assert!(fake.calls().is_empty());
    assert!(fake.downloads().is_empty());
}

#[tokio::test]
async fn a_relative_path_is_refused_before_touching_the_container() {
    let fake = Fake::new();
    let reply = run_op(&fake, &read_op("notes.txt", 0, None, None)).await;
    assert!(is_failed(&reply, FileOpErrorKind::InvalidPath));
    assert!(fake.calls().is_empty());
}

#[tokio::test]
async fn listing_stages_the_directory_and_serves_it_under_its_container_path() -> TestResult {
    let tar = tar_of(&[
        ("src/", None),
        ("src/a.rs", Some(b"fn a() {}")),
        ("src/b.rs", Some(b"fn b() {}")),
    ])?;
    let fake = Fake::new().serving(Download::Tar(tar));
    let reply = run_op(&fake, &list_op("/work/src", FileRecursion::Flat)).await;
    let FileOpResponse::Listing { entries, truncated } = reply else {
        return Err(format!("{reply:?}").into());
    };
    assert!(!truncated.is_truncated());
    let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["a.rs", "b.rs"]);
    assert_eq!(fake.downloads(), ["/work/src"]);
    Ok(())
}

#[tokio::test]
async fn searching_stages_the_tree_and_greps_it() -> TestResult {
    let tar = tar_of(&[
        ("src/", None),
        ("src/a.rs", Some(b"let needle = 1;\nlet other = 2;")),
        ("src/b.rs", Some(b"nothing here")),
    ])?;
    let fake = Fake::new().serving(Download::Tar(tar));
    let op = FileOp::Search {
        path: "/work/src".into(),
        glob: None,
        pattern: Some("needle".into()),
        regex: Default::default(),
        case_insensitive: Default::default(),
        max_results: None,
        max_file_bytes: None,
    };
    let reply = run_op(&fake, &op).await;
    let FileOpResponse::SearchResults { matches, .. } = reply else {
        return Err(format!("{reply:?}").into());
    };
    assert_eq!(matches.len(), 1);
    assert_eq!(matches.first().and_then(|m| m.line), Some(1));
    Ok(())
}

#[tokio::test]
async fn archive_download_packs_the_staged_tree() -> TestResult {
    let tar = tar_of(&[("proj/", None), ("proj/x.txt", Some(b"x"))])?;
    let fake = Fake::new().serving(Download::Tar(tar));
    let op = FileOp::ArchiveGet {
        path: "/work/proj".into(),
        max_bytes: None,
    };
    let reply = run_op(&fake, &op).await;
    let FileOpResponse::Archive { entries, bytes, .. } = reply else {
        return Err(format!("{reply:?}").into());
    };
    assert_eq!((entries, bytes), (1, 1));
    Ok(())
}

#[tokio::test]
async fn a_missing_path_is_not_found_and_an_oversized_tree_is_too_large() {
    let missing = run_op(
        &Fake::new().serving(Download::Missing),
        &list_op("/nope", FileRecursion::Flat),
    )
    .await;
    assert!(matches!(missing, FileOpResponse::NotFound { .. }));

    let big = run_op(
        &Fake::new().serving(Download::TooLarge),
        &list_op("/huge", FileRecursion::Recursive),
    )
    .await;
    assert!(is_failed(&big, FileOpErrorKind::TooLarge));

    let root = run_op(&Fake::new(), &list_op("/", FileRecursion::Recursive)).await;
    assert!(is_failed(&root, FileOpErrorKind::InvalidPath));
}

#[tokio::test]
async fn stat_parses_the_container_stat_line() -> TestResult {
    let fake = Fake::new().replying(0, "regular file|42|644|1700000000\n", "");
    let reply = run_op(
        &fake,
        &FileOp::Stat {
            path: "/a/b.txt".into(),
        },
    )
    .await;
    let FileOpResponse::Stat { stat } = reply else {
        return Err(format!("{reply:?}").into());
    };
    assert_eq!(stat.kind, FileKind::File);
    assert_eq!(stat.size, 42);
    assert_eq!(stat.mode, Some(0o644));
    assert_eq!(stat.modified_unix_ms, Some(1_700_000_000_000));
    assert_eq!(stat.name, "b.txt");

    let gone = Fake::new().replying(44, "", "");
    let reply = run_op(&gone, &FileOp::Stat { path: "/x".into() }).await;
    assert!(matches!(reply, FileOpResponse::NotFound { .. }));
    Ok(())
}

#[tokio::test]
async fn a_stat_line_with_an_unreadable_size_is_refused() {
    let fake = Fake::new().replying(0, "regular file|??|644|1700000000\n", "");
    let reply = run_op(
        &fake,
        &FileOp::Stat {
            path: "/a/b.txt".into(),
        },
    )
    .await;
    assert!(is_failed(&reply, FileOpErrorKind::Io));
}

#[tokio::test]
async fn a_write_whose_resulting_size_is_unreadable_is_refused() -> TestResult {
    let fake = Fake::new().replying(0, "oops\n", "");
    let reply = run_op(&fake, &write_op(b"hello", 0, FileWriteTruncation::Truncate)?).await;
    assert!(is_failed(&reply, FileOpErrorKind::Io));
    Ok(())
}

#[tokio::test]
async fn a_ranged_read_reports_total_size_and_truncation() -> TestResult {
    let fake = Fake::new().replying(0, "10\n", "").replying(0, "cde", "");
    let reply = run_op(&fake, &read_op("/f", 2, Some(3), None)).await;
    assert_eq!(
        reply,
        FileOpResponse::Content {
            data: b64(b"cde")?,
            offset: 2,
            total_size: 10,
            eof: FileReadEnd::More,
            truncated: FileResultTruncation::Complete,
        }
    );
    let calls = fake.calls();
    assert_eq!(
        calls.get(1).map(|c| c.args.clone()),
        Some(vec!["/f".into(), "3".into(), "3".into()])
    );

    let capped = Fake::new().replying(0, "10\n", "").replying(0, "ab", "");
    let reply = run_op(&capped, &read_op("/f", 0, None, Some(2))).await;
    assert!(matches!(
        reply,
        FileOpResponse::Content {
            truncated: FileResultTruncation::Truncated,
            eof: FileReadEnd::More,
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn reading_a_directory_is_a_typed_failure() {
    let fake = Fake::new().replying(21, "", "");
    let reply = run_op(&fake, &read_op("/dir", 0, None, None)).await;
    assert!(is_failed(&reply, FileOpErrorKind::IsADirectory));
}

fn write_op(data: &[u8], offset: u64, truncate: FileWriteTruncation) -> Result<FileOp, Box<dyn Error>> {
    Ok(FileOp::Write {
        path: "/w/out.bin".into(),
        data: b64(data)?,
        offset,
        truncate,
        mode: Some(0o600),
        create_parents: FileParentCreation::Create,
    })
}

fn first_call(fake: &Fake) -> Result<Call, Box<dyn Error>> {
    fake.calls().into_iter().next().ok_or_else(|| "no exec ran".into())
}

#[tokio::test]
async fn a_first_chunk_truncates_and_a_later_chunk_appends() -> TestResult {
    let fake = Fake::new().replying(0, "5\n", "");
    let reply = run_op(&fake, &write_op(b"hello", 0, FileWriteTruncation::Truncate)?).await;
    assert_eq!(reply, FileOpResponse::Written { bytes: 5, size: 5 });
    let call = first_call(&fake)?;
    assert_eq!(call.args, ["/w/out.bin", "0", "1", "600", "1"]);
    assert_eq!(call.stdin.as_deref(), Some(&b"hello"[..]));

    let fake = Fake::new().replying(0, "11\n", "");
    let reply = run_op(&fake, &write_op(b" world", 5, FileWriteTruncation::Keep)?).await;
    assert_eq!(reply, FileOpResponse::Written { bytes: 6, size: 11 });
    assert_eq!(
        first_call(&fake)?.args.get(1..3),
        Some(&["5".to_string(), "0".to_string()][..])
    );
    Ok(())
}

#[tokio::test]
async fn a_write_at_any_other_offset_is_refused_as_non_append() -> TestResult {
    let fake = Fake::new().replying(22, "", "");
    let reply = run_op(&fake, &write_op(b"x", 3, FileWriteTruncation::Keep)?).await;
    assert!(is_failed(&reply, FileOpErrorKind::InvalidRequest));
    Ok(())
}

#[tokio::test]
async fn an_oversized_write_is_refused_without_an_exec() -> TestResult {
    let fake = Fake::new();
    let data = vec![0u8; usize::try_from(FileOp::MAX_WRITE_BYTES + 1)?];
    let reply = run_op(&fake, &write_op(&data, 0, FileWriteTruncation::Truncate)?).await;
    assert!(is_failed(&reply, FileOpErrorKind::TooLarge));
    assert!(fake.calls().is_empty());
    Ok(())
}

fn edit_op(find: &str, replace: &str, expected: u32) -> FileOp {
    FileOp::Edit {
        path: "/w/a.txt".into(),
        find: find.into(),
        replace: replace.into(),
        expected_occurrences: expected,
    }
}

#[tokio::test]
async fn an_edit_with_the_expected_count_writes_the_replacement_back() -> TestResult {
    let tar = tar_of(&[("a.txt", Some(b"a b a"))])?;
    let fake = Fake::new().serving(Download::Tar(tar)).replying(0, "", "");
    let reply = run_op(&fake, &edit_op("a", "z", 2)).await;
    assert_eq!(
        reply,
        FileOpResponse::Edited {
            occurrences: 2,
            size: 5
        }
    );
    let call = first_call(&fake)?;
    assert_eq!(call.stdin.as_deref(), Some(&b"z b z"[..]));
    assert_eq!(call.args, ["/w/a.txt"]);
    assert!(call.script.contains("mv -f"));
    Ok(())
}

#[tokio::test]
async fn an_edit_with_the_wrong_count_writes_nothing() -> TestResult {
    let tar = tar_of(&[("a.txt", Some(b"a b a"))])?;
    let fake = Fake::new().serving(Download::Tar(tar));
    let reply = run_op(&fake, &edit_op("a", "z", 1)).await;
    assert_eq!(
        reply,
        FileOpResponse::EditRejected {
            occurrences: 2,
            expected: 1
        }
    );
    assert!(fake.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn editing_a_missing_file_or_a_directory_is_typed() -> TestResult {
    let reply = run_op(&Fake::new().serving(Download::Missing), &edit_op("a", "b", 1)).await;
    assert!(matches!(reply, FileOpResponse::NotFound { .. }));

    let tar = tar_of(&[("a.txt/", None), ("a.txt/inner", Some(b"x"))])?;
    let reply = run_op(&Fake::new().serving(Download::Tar(tar)), &edit_op("a", "b", 1)).await;
    assert!(is_failed(&reply, FileOpErrorKind::IsADirectory));
    Ok(())
}

fn remove_op(path: &str, recursive: FileRecursion) -> FileOp {
    FileOp::Remove {
        path: path.into(),
        recursive,
    }
}

#[tokio::test]
async fn remove_reports_whether_the_path_existed() -> TestResult {
    let present = Fake::new().replying(0, "", "");
    let reply = run_op(&present, &remove_op("/w/x", FileRecursion::Recursive)).await;
    assert_eq!(
        reply,
        FileOpResponse::Removed {
            existed: FilePresence::Present
        }
    );
    assert_eq!(first_call(&present)?.args, ["/w/x", "1"]);

    let absent = Fake::new().replying(0, "missing\n", "");
    let reply = run_op(&absent, &remove_op("/w/x", FileRecursion::Flat)).await;
    assert_eq!(
        reply,
        FileOpResponse::Removed {
            existed: FilePresence::Absent
        }
    );

    let root = run_op(&Fake::new(), &remove_op("/", FileRecursion::Recursive)).await;
    assert!(is_failed(&root, FileOpErrorKind::InvalidPath));

    let not_empty = Fake::new().replying(1, "", "rmdir: failed to remove: Directory not empty");
    let reply = run_op(&not_empty, &remove_op("/w/d", FileRecursion::Flat)).await;
    assert!(is_failed(&reply, FileOpErrorKind::DirectoryNotEmpty));
    Ok(())
}

fn mkdir_op(path: &str) -> FileOp {
    FileOp::Mkdir {
        path: path.into(),
        parents: FileParentCreation::Create,
    }
}

#[tokio::test]
async fn mkdir_reports_an_existing_directory_and_refuses_a_file() {
    let fresh = Fake::new().replying(0, "", "");
    let reply = run_op(&fresh, &mkdir_op("/w/new")).await;
    assert_eq!(
        reply,
        FileOpResponse::Created {
            already_existed: FilePresence::Absent
        }
    );

    let existing = Fake::new().replying(0, "exists\n", "");
    let reply = run_op(&existing, &mkdir_op("/w/new")).await;
    assert_eq!(
        reply,
        FileOpResponse::Created {
            already_existed: FilePresence::Present
        }
    );

    let file = Fake::new().replying(17, "", "");
    let reply = run_op(&file, &mkdir_op("/w/file")).await;
    assert!(is_failed(&reply, FileOpErrorKind::AlreadyExists));
}

#[tokio::test]
async fn rename_maps_missing_sources_and_existing_targets() -> TestResult {
    let op = FileOp::Rename {
        from: "/w/a".into(),
        to: "/w/b".into(),
        overwrite: FileOverwrite::Refuse,
    };
    let ok = Fake::new().replying(0, "", "");
    assert_eq!(run_op(&ok, &op).await, FileOpResponse::Renamed);
    assert_eq!(first_call(&ok)?.args, ["/w/a", "/w/b", "0"]);

    let missing = Fake::new().replying(44, "", "");
    assert!(matches!(run_op(&missing, &op).await, FileOpResponse::NotFound { .. }));

    let exists = Fake::new().replying(17, "", "");
    assert!(is_failed(&run_op(&exists, &op).await, FileOpErrorKind::AlreadyExists));
    Ok(())
}

#[tokio::test]
async fn archive_put_validates_before_extracting_and_needs_tar_in_the_image() -> TestResult {
    let bad = Fake::new();
    let op = FileOp::ArchivePut {
        path: "/w/dest".into(),
        data: b64(b"not a tar")?,
    };
    let reply = run_op(&bad, &op).await;
    assert!(is_failed(&reply, FileOpErrorKind::InvalidRequest));
    assert!(bad.calls().is_empty());

    let data = tar_of(&[("d/", None), ("d/f.txt", Some(b"hi"))])?;
    let put = FileOp::ArchivePut {
        path: "/w/dest".into(),
        data: b64(&data)?,
    };
    let ok = Fake::new().replying(0, "", "");
    let reply = run_op(&ok, &put).await;
    assert_eq!(reply, FileOpResponse::Extracted { entries: 2, bytes: 2 });
    assert_eq!(first_call(&ok)?.stdin.as_deref(), Some(&data[..]));

    let no_tar = Fake::new().replying(127, "", "tar: not found");
    let reply = run_op(&no_tar, &put).await;
    assert!(matches!(reply, FileOpResponse::Unsupported { .. }));
    Ok(())
}

#[tokio::test]
async fn container_source_parses_stat_lines_and_keeps_pipes_in_names() -> TestResult {
    let listing = "directory|4096|2026-01-01 00:00:00.000000000 +0000|/w/sub\n\
                   regular file|5|2026-01-01 00:00:01.500000000 +0000|/w/a|b.txt\n\
                   symbolic link|3|2026-01-01 00:00:02.000000000 +0000|/w/link\n";
    let fake = Fake::new().replying(0, listing, "");
    let source = source(&fake);
    let manifest = source
        .manifest("/w", FileRecursion::Recursive, FileContentHashing::Metadata)
        .await?;
    assert_eq!(manifest.len(), 3);
    let sub = manifest.get("/w/sub").ok_or("no /w/sub")?;
    assert!(sub.is_directory() && sub.stamp().is_none());
    let file = manifest.get("/w/a|b.txt").ok_or("no /w/a|b.txt")?;
    assert_eq!((file.is_directory(), file.size()), (false, 5));
    assert!(file.stamp().ok_or("no stamp")?.contains("01.5"));
    assert!(!manifest.get("/w/link").ok_or("no /w/link")?.is_directory());
    let call = first_call(&fake)?;
    assert_eq!(call.args.first().map(String::as_str), Some("/w"));
    assert_eq!(call.args.get(1).map(String::as_str), Some("1"));
    Ok(())
}

#[tokio::test]
async fn container_source_adds_content_hashes_only_when_asked() -> TestResult {
    let listing = "regular file|5|2026-01-01 00:00:01.000000000 +0000|/w/a.txt\n";
    let digest = "a".repeat(64);
    let fake = Fake::new()
        .replying(0, listing, "")
        .replying(0, &format!("{digest}  /w/a.txt\n"), "");
    let source = source(&fake);
    let manifest = source
        .manifest("/w", FileRecursion::Flat, FileContentHashing::Content)
        .await?;
    let entry = manifest.get("/w/a.txt").ok_or("no /w/a.txt")?;
    assert_eq!(entry.hash().map(FileSha256::as_str), Some(digest.as_str()));
    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls.get(1).and_then(|c| c.args.get(1)).map(String::as_str), Some("0"));

    let no_sha = Fake::new().replying(0, listing, "").replying(127, "", "");
    let source = self::source(&no_sha);
    assert!(matches!(
        source
            .manifest("/w", FileRecursion::Flat, FileContentHashing::Content)
            .await,
        Err(FileOpError::Unsupported { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn container_source_refuses_the_root_a_missing_path_and_an_oversized_tree() {
    let fake = Fake::new();
    let scan = source(&fake);
    assert!(matches!(
        scan.manifest("/", FileRecursion::Recursive, FileContentHashing::Metadata)
            .await,
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::InvalidPath,
            ..
        })
    ));
    assert!(matches!(
        scan.manifest("relative", FileRecursion::Recursive, FileContentHashing::Metadata)
            .await,
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::InvalidPath,
            ..
        })
    ));
    assert!(fake.calls().is_empty());

    let missing = Fake::new().replying(44, "", "");
    assert!(matches!(
        source(&missing)
            .manifest("/nowhere", FileRecursion::Recursive, FileContentHashing::Metadata)
            .await,
        Err(FileOpError::NotFound { .. })
    ));

    let line = "regular file|1|2026-01-01 00:00:00.000000000 +0000|/w/f";
    let many: String = (0..=FileOp::TRACKING_MAX_ENTRIES)
        .map(|i| format!("{line}{i}\n"))
        .collect();
    let big = Fake::new().replying(0, &many, "");
    assert!(matches!(
        source(&big)
            .manifest("/w", FileRecursion::Recursive, FileContentHashing::Metadata)
            .await,
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::TooLarge,
            ..
        })
    ));
}

#[tokio::test]
async fn tracking_verbs_through_the_stateless_path_are_unsupported_and_changes_need_a_baseline() -> TestResult {
    let fake = Fake::new();
    let reply = run_op(&fake, &changes_op(None)).await;
    assert!(matches!(reply, FileOpResponse::Unsupported { .. }));

    // No docker diff and no stored baseline: the caller is told to checkpoint first.
    let tracking = Tracking::new();
    let reply = tracking
        .serve("l1", &changes_op(None), &source(&fake))
        .await
        .ok_or("no reply")?;
    assert!(matches!(
        reply,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::InvalidRequest,
            ..
        }
    ));
    Ok(())
}

struct WithDiff(Fake);

impl ContainerAccess for WithDiff {
    async fn download(&self, path: &str, cap: u64) -> Download {
        self.0.download(path, cap).await
    }

    async fn sh(&self, script: &str, args: &[&str], stdin: Option<Vec<u8>>) -> anyhow::Result<CommandOutput> {
        self.0.sh(script, args, stdin).await
    }

    async fn changes_since_creation(&self) -> Option<Result<Vec<(String, u8)>, String>> {
        Some(Ok(vec![
            ("/w/new".into(), 1),
            ("/w".into(), 0),
            ("/w/old".into(), 2),
            ("/etc/hosts".into(), 0),
            ("/workspace".into(), 0),
            ("/workspace/from-diff".into(), 1),
        ]))
    }
}

fn changes(since: &str, added: &[&str], modified: &[&str], deleted: &[&str]) -> FileOpResponse {
    let owned = |paths: &[&str]| paths.iter().map(|p| (*p).to_string()).collect();
    FileOpResponse::Changes {
        since: since.into(),
        added: owned(added),
        modified: owned(modified),
        deleted: owned(deleted),
        truncated: FileResultTruncation::Complete,
    }
}

#[tokio::test]
async fn changes_without_a_checkpoint_use_the_container_diff_narrowed_to_the_path() -> TestResult {
    let container = WithDiff(Fake::new());
    let tracking = Tracking::new();
    let reply = tracking
        .serve("l1", &changes_op(Some("/w")), &source(&container))
        .await
        .ok_or("no reply")?;
    assert_eq!(reply, changes("start", &["/w/new"], &["/w"], &["/w/old"]));
    Ok(())
}

#[tokio::test]
async fn a_checksum_hashes_the_whole_file_or_a_range_through_ranged_reads() -> TestResult {
    let checksum_op = |path: &str, offset, length| FileOp::Checksum {
        path: path.into(),
        offset,
        length,
    };
    let whole = Fake::new().replying(0, "5\n", "").replying(0, "hello", "");
    let reply = run_op(&whole, &checksum_op("/f", 0, None)).await;
    assert_eq!(
        reply,
        FileOpResponse::Checksum {
            sha256: trogon_sandbox_file_ops::content_hash(b"hello")?,
            bytes: 5,
            total_size: 5,
        }
    );

    let ranged = Fake::new().replying(0, "5\n", "").replying(0, "ell", "");
    let reply = run_op(&ranged, &checksum_op("/f", 1, Some(3))).await;
    assert_eq!(
        reply,
        FileOpResponse::Checksum {
            sha256: trogon_sandbox_file_ops::content_hash(b"ell")?,
            bytes: 3,
            total_size: 5,
        }
    );

    let missing = Fake::new().replying(44, "", "");
    let reply = run_op(&missing, &checksum_op("/nope", 0, None)).await;
    assert!(matches!(reply, FileOpResponse::NotFound { .. }));
    Ok(())
}

fn stat_line(kind: &str, size: u64, stamp: &str, name: &str) -> String {
    format!("{kind}|{size}|2024-01-01 00:00:0{stamp}.000000000 +0000|{name}\n")
}

#[tokio::test]
async fn changes_without_a_checkpoint_compare_the_workspace_with_the_baseline_taken_at_lease_start() -> TestResult {
    let at_start = [
        stat_line("directory", 4096, "0", "/workspace/src"),
        stat_line("regular file", 3, "0", "/workspace/src/keep.rs"),
        stat_line("regular file", 3, "0", "/workspace/src/edit.rs"),
        stat_line("regular file", 3, "0", "/workspace/gone.txt"),
    ]
    .concat();
    let now = [
        stat_line("directory", 4096, "0", "/workspace/src"),
        stat_line("regular file", 3, "0", "/workspace/src/keep.rs"),
        stat_line("regular file", 9, "5", "/workspace/src/edit.rs"),
        stat_line("regular file", 1, "6", "/workspace/new.txt"),
    ]
    .concat();
    // Inside /workspace the baseline wins over the container diff; outside it, the
    // container diff is all there is.
    let container = WithDiff(
        Fake::new()
            .replying(0, &at_start, "")
            .replying(0, &now, "")
            .replying(0, &now, ""),
    );
    let source = source(&container);
    let tracking = Tracking::new();
    record_start_baseline(&tracking, "l1", &source).await;

    let reply = tracking
        .serve("l1", &changes_op(None), &source)
        .await
        .ok_or("no reply")?;
    assert_eq!(
        reply,
        changes(
            "start",
            &["/w/new", "/workspace/new.txt"],
            &["/etc/hosts", "/w", "/workspace/src/edit.rs"],
            &["/w/old", "/workspace/gone.txt"],
        )
    );
    let inside = tracking
        .serve("l1", &changes_op(Some("/workspace/src")), &source)
        .await
        .ok_or("no reply")?;
    assert_eq!(inside, changes("start", &[], &["/workspace/src/edit.rs"], &[]));
    let calls = container.0.calls();
    assert_eq!(calls.len(), 3);
    assert!(
        calls
            .iter()
            .all(|c| c.args.first().map(String::as_str) == Some("/workspace"))
    );
    Ok(())
}

#[tokio::test]
async fn a_container_without_a_workspace_keeps_no_baseline_and_falls_back_to_the_container_diff() -> TestResult {
    let container = WithDiff(Fake::new().replying(44, "", ""));
    let source = source(&container);
    let tracking = Tracking::new();
    record_start_baseline(&tracking, "l1", &source).await;

    let reply = tracking
        .serve("l1", &changes_op(Some("/w")), &source)
        .await
        .ok_or("no reply")?;
    assert_eq!(reply, changes("start", &["/w/new"], &["/w"], &["/w/old"]));
    Ok(())
}

#[tokio::test]
async fn a_failed_baseline_scan_clears_an_earlier_baseline_instead_of_keeping_a_stale_one() -> TestResult {
    let first = stat_line("regular file", 1, "0", "/workspace/a");
    let container = Fake::new().replying(0, &first, "").replying(1, "", "boom");
    let source = source(&container);
    let tracking = Tracking::new();
    record_start_baseline(&tracking, "l1", &source).await;
    record_start_baseline(&tracking, "l1", &source).await;

    let reply = tracking
        .serve("l1", &changes_op(None), &source)
        .await
        .ok_or("no reply")?;
    assert!(is_failed(&reply, FileOpErrorKind::InvalidRequest));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_watch_wait_without_inotifywait_falls_back_to_the_scan_interval() -> TestResult {
    let container = Fake::new().replying(127, "", "");
    let source = source(&container);
    let started = tokio::time::Instant::now();
    source
        .wait_for_change(
            "/workspace/../workspace",
            FileRecursion::Recursive,
            Duration::from_secs(30),
        )
        .await;
    assert_eq!(started.elapsed(), source.scan_interval());
    let calls = container.calls();
    assert_eq!(calls.len(), 1);
    let call = first_call(&container)?;
    assert!(call.script.contains("inotifywait"));
    assert_eq!(call.args, ["/workspace", "1", "5"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_watch_wait_returns_as_soon_as_inotifywait_reports_a_change() -> TestResult {
    let container = Fake::new().replying(0, "", "");
    let source = source(&container);
    let started = tokio::time::Instant::now();
    source
        .wait_for_change("/workspace", FileRecursion::Flat, Duration::from_secs(3))
        .await;
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(first_call(&container)?.args, ["/workspace", "0", "3"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_sub_second_watch_wait_sleeps_without_running_inotifywait() {
    let container = Fake::new();
    let source = source(&container);
    let started = tokio::time::Instant::now();
    source
        .wait_for_change("/workspace", FileRecursion::Recursive, Duration::from_millis(400))
        .await;
    assert_eq!(started.elapsed(), Duration::from_millis(400));
    assert!(container.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_lease_without_inotifywait_skips_the_probe_on_later_waits() {
    let container = Fake::new().replying(127, "", "");
    let missing = InotifyMissing::default();
    let scan = ContainerSource {
        container: &container,
        inotify_missing: Some((&missing, "l1")),
        feeds: None,
    };
    scan.wait_for_change("/workspace", FileRecursion::Recursive, Duration::from_secs(30))
        .await;
    let started = tokio::time::Instant::now();
    scan.wait_for_change("/workspace", FileRecursion::Recursive, Duration::from_secs(30))
        .await;
    assert_eq!(started.elapsed(), scan.scan_interval());
    assert_eq!(container.calls().len(), 1);

    missing.forget("l1");
    let other = Fake::new().replying(0, "", "");
    let scan = ContainerSource {
        container: &other,
        inotify_missing: Some((&missing, "l1")),
        feeds: None,
    };
    scan.wait_for_change("/workspace", FileRecursion::Recursive, Duration::from_secs(30))
        .await;
    assert_eq!(other.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_watch_with_a_live_feed_waits_on_it_instead_of_running_inotifywait() {
    let container = Fake::new().replying(0, "", "");
    let feeds = feed::WatchFeeds::default();
    let live = feeds.insert_for_test("w1", "l1");
    let source = ContainerSource {
        container: &container,
        inotify_missing: None,
        feeds: Some(&feeds),
    };

    live.push_for_test("/workspace/tmp", FileWatchEventKind::Created);
    let started = tokio::time::Instant::now();
    source
        .wait_for_watch("w1", "/workspace", FileRecursion::Recursive, Duration::from_secs(30))
        .await;
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert!(container.calls().is_empty());
    assert_eq!(source.drain_hints("w1").len(), 1);
    assert!(source.drain_hints("w1").is_empty());

    live.end_for_test();
    source
        .wait_for_watch("w1", "/workspace", FileRecursion::Recursive, Duration::from_secs(30))
        .await;
    assert_eq!(
        container.calls().len(),
        1,
        "a feed that ended falls back to the one-shot wait"
    );
}
