use trogon_sandbox_contract::{
    Base64Data, FileCaseSensitivity, FileContentHashing, FileOp, FileOverwrite, FileParentCreation, FileRecursion,
    FileSearchSyntax, FileWriteTruncation,
};

use super::file_op_paths_mut;
use crate::testing::TestResult;

fn paths(mut op: FileOp) -> Vec<String> {
    file_op_paths_mut(&mut op)
        .into_iter()
        .map(|path| path.clone())
        .collect()
}

#[test]
fn single_path_operations_name_their_path() -> TestResult {
    let data = Base64Data::new("")?;
    let ops = [
        FileOp::List {
            path: "p".into(),
            recursive: FileRecursion::default(),
            max_entries: None,
        },
        FileOp::Stat { path: "p".into() },
        FileOp::Read {
            path: "p".into(),
            offset: 0,
            length: None,
            max_bytes: None,
        },
        FileOp::Write {
            path: "p".into(),
            data: data.clone(),
            offset: 0,
            truncate: FileWriteTruncation::default(),
            mode: None,
            create_parents: FileParentCreation::default(),
        },
        FileOp::Edit {
            path: "p".into(),
            find: "a".into(),
            replace: "b".into(),
            expected_occurrences: 1,
        },
        FileOp::Search {
            path: "p".into(),
            glob: None,
            pattern: None,
            regex: FileSearchSyntax::default(),
            case_insensitive: FileCaseSensitivity::default(),
            max_results: None,
            max_file_bytes: None,
        },
        FileOp::Remove {
            path: "p".into(),
            recursive: FileRecursion::default(),
        },
        FileOp::Mkdir {
            path: "p".into(),
            parents: FileParentCreation::default(),
        },
        FileOp::ArchiveGet {
            path: "p".into(),
            max_bytes: None,
        },
        FileOp::ArchivePut { path: "p".into(), data },
        FileOp::Checkpoint {
            path: "p".into(),
            hash: FileContentHashing::default(),
        },
        FileOp::Checksum {
            path: "p".into(),
            offset: 0,
            length: None,
        },
        FileOp::WatchStart {
            path: "p".into(),
            recursive: FileRecursion::default(),
        },
    ];
    for op in ops {
        assert_eq!(paths(op), ["p"]);
    }
    Ok(())
}

#[test]
fn changes_names_its_optional_path() {
    assert_eq!(
        paths(FileOp::Changes {
            since: None,
            path: Some("p".into())
        }),
        ["p"]
    );
    assert!(
        paths(FileOp::Changes {
            since: None,
            path: None
        })
        .is_empty()
    );
}

#[test]
fn rename_names_both_ends() {
    let op = FileOp::Rename {
        from: "a".into(),
        to: "b".into(),
        overwrite: FileOverwrite::default(),
    };
    assert_eq!(paths(op), ["a", "b"]);
}

#[test]
fn watch_polls_and_stops_name_no_path() {
    assert!(
        paths(FileOp::WatchPoll {
            watch_id: "w".into(),
            after_seq: 0,
            wait_ms: None
        })
        .is_empty()
    );
    assert!(paths(FileOp::WatchStop { watch_id: "w".into() }).is_empty());
}
