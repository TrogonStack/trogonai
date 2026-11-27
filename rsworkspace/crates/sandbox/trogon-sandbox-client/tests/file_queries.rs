mod common;
mod file_support;

use common::{TestResult, lease_id};
use file_support::{client_for, sent_op};
use trogon_sandbox_client::{ClientError, FileRange};
use trogon_sandbox_contract::{
    Base64Data, FileCaseSensitivity, FileKind, FileOp, FileOpResponse, FileOverwrite, FileParentCreation, FilePresence,
    FileReadEnd, FileRecursion, FileResultTruncation, FileSearchSyntax, FileStat, SandboxUserName, SearchMatch,
};

fn stat() -> FileStat {
    FileStat {
        path: "/w/a".into(),
        name: "a".into(),
        kind: FileKind::File,
        size: 3,
        mode: Some(0o644),
        modified_unix_ms: None,
        symlink_target: None,
    }
}

#[tokio::test]
async fn list_dir_returns_entries_and_truncation() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Listing {
        entries: vec![stat()],
        truncated: FileResultTruncation::Truncated,
    })?;
    let user = SandboxUserName::new("builder")?;
    let (entries, truncated) = client
        .list_dir(&lease_id(), "/w", FileRecursion::Recursive, Some(5), Some(user))
        .await?;
    assert_eq!(entries, vec![stat()]);
    assert_eq!(truncated, FileResultTruncation::Truncated);
    assert_eq!(
        sent_op(&log)?,
        FileOp::List {
            path: "/w".into(),
            recursive: FileRecursion::Recursive,
            max_entries: Some(5)
        }
    );
    Ok(())
}

#[tokio::test]
async fn stat_returns_the_entry() -> TestResult {
    let (client, _) = client_for(&FileOpResponse::Stat { stat: stat() })?;
    assert_eq!(client.stat(&lease_id(), "/w/a", None).await?, stat());
    Ok(())
}

#[tokio::test]
async fn read_range_returns_total_size_and_truncation() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Content {
        data: Base64Data::new("YWI=")?,
        offset: 4,
        total_size: 100,
        eof: FileReadEnd::More,
        truncated: FileResultTruncation::Truncated,
    })?;
    let range = client.read_range(&lease_id(), "/a", 4, None, Some(2), None).await?;
    assert_eq!(
        range,
        FileRange {
            data: b"ab".to_vec(),
            offset: 4,
            total_size: 100,
            eof: FileReadEnd::More,
            truncated: FileResultTruncation::Truncated
        }
    );
    assert_eq!(
        sent_op(&log)?,
        FileOp::Read {
            path: "/a".into(),
            offset: 4,
            length: None,
            max_bytes: Some(2)
        }
    );
    Ok(())
}

#[tokio::test]
async fn edit_file_returns_the_replacement_count() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Edited {
        occurrences: 2,
        size: 10,
    })?;
    assert_eq!(client.edit_file(&lease_id(), "/a", "x", "y", 2, None).await?, 2);
    assert_eq!(
        sent_op(&log)?,
        FileOp::Edit {
            path: "/a".into(),
            find: "x".into(),
            replace: "y".into(),
            expected_occurrences: 2
        }
    );
    Ok(())
}

#[tokio::test]
async fn edit_file_reports_a_rejected_edit_as_an_error() -> TestResult {
    let (client, _) = client_for(&FileOpResponse::EditRejected {
        occurrences: 3,
        expected: 1,
    })?;
    let error = client.edit_file(&lease_id(), "/a", "x", "y", 1, None).await.err();
    assert!(matches!(
        error,
        Some(ClientError::EditRejected {
            occurrences: 3,
            expected: 1
        })
    ));
    Ok(())
}

#[tokio::test]
async fn search_returns_matches() -> TestResult {
    let found = SearchMatch {
        path: "/w/a".into(),
        line: Some(1),
        text: Some("hit".into()),
    };
    let (client, _) = client_for(&FileOpResponse::SearchResults {
        matches: vec![found.clone()],
        truncated: FileResultTruncation::Complete,
        files_scanned: 7,
    })?;
    let op = FileOp::Search {
        path: "/w".into(),
        glob: None,
        pattern: Some("hit".into()),
        regex: FileSearchSyntax::default(),
        case_insensitive: FileCaseSensitivity::default(),
        max_results: None,
        max_file_bytes: None,
    };
    let outcome = client.search(&lease_id(), op, None).await?;
    assert_eq!(outcome.matches, vec![found]);
    assert_eq!(outcome.files_scanned, 7);
    assert_eq!(outcome.truncated, FileResultTruncation::Complete);
    Ok(())
}

#[tokio::test]
async fn remove_path_reports_whether_it_existed() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Removed {
        existed: FilePresence::Present,
    })?;
    let existed = client
        .remove_path(&lease_id(), "/a", FileRecursion::Recursive, None)
        .await?;
    assert_eq!(existed, FilePresence::Present);
    assert_eq!(
        sent_op(&log)?,
        FileOp::Remove {
            path: "/a".into(),
            recursive: FileRecursion::Recursive
        }
    );
    Ok(())
}

#[tokio::test]
async fn mkdir_creates_a_directory() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Created {
        already_existed: FilePresence::Absent,
    })?;
    client
        .mkdir(&lease_id(), "/a", FileParentCreation::Create, None)
        .await?;
    assert_eq!(
        sent_op(&log)?,
        FileOp::Mkdir {
            path: "/a".into(),
            parents: FileParentCreation::Create
        }
    );
    Ok(())
}

#[tokio::test]
async fn rename_path_sends_the_request_on_the_file_op_subject() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Renamed)?;
    client
        .rename_path(&lease_id(), "/a", "/b", FileOverwrite::Overwrite, None)
        .await?;
    assert_eq!(log.only()?.subject, "sandbox.lease.file_op.lease-1");
    assert_eq!(
        sent_op(&log)?,
        FileOp::Rename {
            from: "/a".into(),
            to: "/b".into(),
            overwrite: FileOverwrite::Overwrite
        }
    );
    Ok(())
}

#[tokio::test]
async fn every_query_rejects_a_reply_of_the_wrong_kind() -> TestResult {
    let (client, _) = client_for(&FileOpResponse::Renamed)?;
    let id = lease_id();
    assert!(
        client
            .list_dir(&id, "/", FileRecursion::Flat, None, None)
            .await
            .is_err()
    );
    assert!(client.read_range(&id, "/", 0, None, None, None).await.is_err());
    assert!(client.edit_file(&id, "/", "a", "b", 1, None).await.is_err());
    let search = FileOp::Stat { path: "/".into() };
    assert!(client.search(&id, search, None).await.is_err());
    assert!(client.remove_path(&id, "/", FileRecursion::Flat, None).await.is_err());
    assert!(client.mkdir(&id, "/", FileParentCreation::Require, None).await.is_err());
    let (client, _) = client_for(&FileOpResponse::Stat { stat: stat() })?;
    assert!(
        client
            .rename_path(&id, "/", "/b", FileOverwrite::Refuse, None)
            .await
            .is_err()
    );
    Ok(())
}
