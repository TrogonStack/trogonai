use std::error::Error;
use std::fs;
use std::io::Read as _;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tempfile::TempDir;
use trogon_sandbox_contract::{
    Base64Data, FileCaseSensitivity, FileContentHashing, FileKind, FileOp, FileOpErrorKind, FileOpResponse,
    FileOverwrite, FileParentCreation, FilePresence, FileRecursion, FileResultTruncation, FileSearchSyntax,
    FileWriteTruncation,
};
use trogon_sandbox_file_ops::{FileOpFailure, FsRoot, SandboxPrefix};

type TestResult = Result<(), Box<dyn Error>>;

fn root() -> Result<(TempDir, FsRoot), Box<dyn Error>> {
    let dir = TempDir::new()?;
    let fs_root = FsRoot::new(dir.path(), Some(SandboxPrefix::new("/workspace")?))?;
    Ok((dir, fs_root))
}

fn data(bytes: &[u8]) -> Result<Base64Data, Box<dyn Error>> {
    Ok(Base64Data::new(STANDARD.encode(bytes))?)
}

fn bytes_of(response: FileOpResponse) -> Result<Vec<u8>, Box<dyn Error>> {
    match response {
        FileOpResponse::Content { data, .. } => Ok(STANDARD.decode(data.as_str())?),
        other => Err(format!("expected content, got {other:?}").into()),
    }
}

fn write_op(path: &str, bytes: &[u8]) -> Result<FileOp, Box<dyn Error>> {
    Ok(FileOp::Write {
        path: path.into(),
        data: data(bytes)?,
        offset: 0,
        truncate: FileWriteTruncation::Truncate,
        mode: None,
        create_parents: FileParentCreation::Create,
    })
}

fn read_op(path: &str, offset: u64, length: Option<u64>, max_bytes: Option<u64>) -> FileOp {
    FileOp::Read {
        path: path.into(),
        offset,
        length,
        max_bytes,
    }
}

fn edit_op(path: &str, find: &str, replace: &str, expected: u32) -> FileOp {
    FileOp::Edit {
        path: path.into(),
        find: find.into(),
        replace: replace.into(),
        expected_occurrences: expected,
    }
}

fn list_op(path: &str, recursive: FileRecursion, max_entries: Option<u32>) -> FileOp {
    FileOp::List {
        path: path.into(),
        recursive,
        max_entries,
    }
}

fn search_op(path: &str, glob: Option<&str>, pattern: Option<&str>) -> FileOp {
    FileOp::Search {
        path: path.into(),
        glob: glob.map(String::from),
        pattern: pattern.map(String::from),
        regex: FileSearchSyntax::Literal,
        case_insensitive: FileCaseSensitivity::Sensitive,
        max_results: None,
        max_file_bytes: None,
    }
}

fn is_failure(response: &FileOpResponse, wanted: FileOpErrorKind) -> bool {
    matches!(response, FileOpResponse::Failed { kind, .. } if *kind == wanted)
}

#[test]
fn write_then_read_round_trips_and_creates_parents() -> TestResult {
    let (dir, fs_root) = root()?;
    let reply = fs_root.execute(&write_op("/workspace/a/b/c.txt", b"hello")?);
    assert_eq!(reply, FileOpResponse::Written { bytes: 5, size: 5 });
    assert_eq!(fs::read(dir.path().join("a/b/c.txt"))?, b"hello");
    assert_eq!(
        bytes_of(fs_root.execute(&read_op("a/b/c.txt", 0, None, None)))?,
        b"hello"
    );
    Ok(())
}

#[test]
fn write_without_create_parents_fails_when_the_parent_is_missing() -> TestResult {
    let (_dir, fs_root) = root()?;
    let reply = fs_root.execute(&FileOp::Write {
        path: "x/y.txt".into(),
        data: data(b"1")?,
        offset: 0,
        truncate: FileWriteTruncation::Truncate,
        mode: None,
        create_parents: FileParentCreation::Require,
    });
    assert!(matches!(reply, FileOpResponse::NotFound { .. }), "{reply:?}");
    Ok(())
}

#[test]
fn chunked_write_assembles_the_file_in_order() -> TestResult {
    let (dir, fs_root) = root()?;
    assert_eq!(
        fs_root.execute(&write_op("big.bin", b"AAAA")?),
        FileOpResponse::Written { bytes: 4, size: 4 }
    );
    let second = fs_root.execute(&FileOp::Write {
        path: "big.bin".into(),
        data: data(b"BBBB")?,
        offset: 4,
        truncate: FileWriteTruncation::Keep,
        mode: None,
        create_parents: FileParentCreation::Create,
    });
    assert_eq!(second, FileOpResponse::Written { bytes: 4, size: 8 });
    assert_eq!(fs::read(dir.path().join("big.bin"))?, b"AAAABBBB");
    Ok(())
}

#[test]
fn a_write_without_truncation_creates_a_missing_file() -> TestResult {
    let (dir, fs_root) = root()?;
    let reply = fs_root.execute(&FileOp::Write {
        path: "new.txt".into(),
        data: data(b"abc")?,
        offset: 0,
        truncate: FileWriteTruncation::Keep,
        mode: Some(0o600),
        create_parents: FileParentCreation::Create,
    });
    assert_eq!(reply, FileOpResponse::Written { bytes: 3, size: 3 });
    assert_eq!(fs::read(dir.path().join("new.txt"))?, b"abc");
    Ok(())
}

#[test]
fn a_ranged_write_that_truncates_cuts_the_file_first() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("t.txt"), b"0123456789")?;
    let reply = fs_root.execute(&FileOp::Write {
        path: "t.txt".into(),
        data: data(b"ab")?,
        offset: 2,
        truncate: FileWriteTruncation::Truncate,
        mode: None,
        create_parents: FileParentCreation::Create,
    });
    assert_eq!(reply, FileOpResponse::Written { bytes: 2, size: 4 });
    Ok(())
}

#[test]
fn a_write_past_the_request_limit_is_refused() -> TestResult {
    let (_dir, fs_root) = root()?;
    let big = vec![0u8; FileOp::MAX_WRITE_BYTES as usize + 1];
    let reply = fs_root.execute(&write_op("big.bin", &big)?);
    assert!(is_failure(&reply, FileOpErrorKind::TooLarge), "{reply:?}");
    Ok(())
}

#[test]
fn writing_the_root_or_a_directory_is_refused() -> TestResult {
    let (dir, fs_root) = root()?;
    let reply = fs_root.execute(&write_op("/workspace", b"x")?);
    assert!(is_failure(&reply, FileOpErrorKind::IsADirectory), "{reply:?}");
    fs::create_dir(dir.path().join("d"))?;
    let reply = fs_root.execute(&write_op("d", b"x")?);
    assert!(matches!(reply, FileOpResponse::Failed { .. }), "{reply:?}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_whole_file_write_swaps_in_a_new_file_and_keeps_the_mode() -> TestResult {
    let (dir, fs_root) = root()?;
    let host = dir.path().join("conf.txt");
    fs::write(&host, b"old content")?;
    fs::set_permissions(&host, fs::Permissions::from_mode(0o640))?;
    let before = fs::metadata(&host)?;
    let mut reader = fs::File::open(&host)?;

    let reply = fs_root.execute(&write_op("conf.txt", b"new")?);
    assert_eq!(reply, FileOpResponse::Written { bytes: 3, size: 3 });

    let mut seen = Vec::new();
    reader.read_to_end(&mut seen)?;
    assert_eq!(seen, b"old content");
    let after = fs::metadata(&host)?;
    assert_ne!(after.ino(), before.ino());
    assert_eq!(after.permissions().mode() & 0o777, 0o640);
    assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
    assert_eq!(fs::read(&host)?, b"new");
    let names = fs::read_dir(dir.path())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(names.len(), 1, "temp files left behind: {names:?}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_whole_file_write_mode_overrides_the_existing_mode() -> TestResult {
    let (dir, fs_root) = root()?;
    let host = dir.path().join("run.sh");
    fs::write(&host, b"x")?;
    fs::set_permissions(&host, fs::Permissions::from_mode(0o600))?;
    fs_root.execute(&FileOp::Write {
        path: "run.sh".into(),
        data: data(b"#!/bin/sh\n")?,
        offset: 0,
        truncate: FileWriteTruncation::Truncate,
        mode: Some(0o750),
        create_parents: FileParentCreation::Create,
    });
    assert_eq!(fs::metadata(&host)?.permissions().mode() & 0o777, 0o750);
    fs_root.execute(&FileOp::Write {
        path: "fresh.sh".into(),
        data: data(b"x")?,
        offset: 0,
        truncate: FileWriteTruncation::Truncate,
        mode: Some(0o700),
        create_parents: FileParentCreation::Create,
    });
    assert_eq!(
        fs::metadata(dir.path().join("fresh.sh"))?.permissions().mode() & 0o777,
        0o700
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_write_never_follows_a_symlink() -> TestResult {
    let (dir, fs_root) = root()?;
    let outside = TempDir::new()?;
    fs::write(outside.path().join("secret"), b"keep")?;
    std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("link"))?;
    let reply = fs_root.execute(&write_op("link", b"pwn")?);
    assert!(matches!(reply, FileOpResponse::Failed { .. }), "{reply:?}");
    let ranged = fs_root.execute(&FileOp::Write {
        path: "link".into(),
        data: data(b"pwn")?,
        offset: 1,
        truncate: FileWriteTruncation::Keep,
        mode: None,
        create_parents: FileParentCreation::Create,
    });
    assert!(matches!(ranged, FileOpResponse::Failed { .. }), "{ranged:?}");
    assert_eq!(fs::read(outside.path().join("secret"))?, b"keep");
    Ok(())
}

#[test]
fn read_honours_offset_length_and_the_byte_cap() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("f.txt"), b"0123456789")?;
    let reply = fs_root.execute(&read_op("f.txt", 2, Some(4), None));
    assert!(
        matches!(
            &reply,
            FileOpResponse::Content {
                offset: 2,
                total_size: 10,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(bytes_of(reply)?, b"2345");

    let capped = fs_root.execute(&read_op("f.txt", 0, None, Some(3)));
    assert!(
        matches!(
            &capped,
            FileOpResponse::Content { truncated, .. } if *truncated == FileResultTruncation::Truncated
        ),
        "{capped:?}"
    );
    assert_eq!(bytes_of(capped)?, b"012");

    let past = fs_root.execute(&read_op("f.txt", 99, None, None));
    assert_eq!(bytes_of(past)?, b"");
    Ok(())
}

#[test]
fn read_failures_name_the_cause() -> TestResult {
    let (dir, fs_root) = root()?;
    assert!(matches!(
        fs_root.execute(&read_op("missing", 0, None, None)),
        FileOpResponse::NotFound { .. }
    ));
    fs::create_dir(dir.path().join("d"))?;
    assert!(matches!(
        fs_root.execute(&read_op("d", 0, None, None)),
        FileOpResponse::Failed { .. }
    ));
    Ok(())
}

#[test]
fn checksum_hashes_the_requested_range() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("f.txt"), b"abcdef")?;
    let whole = fs_root.execute(&FileOp::Checksum {
        path: "f.txt".into(),
        offset: 0,
        length: None,
    });
    let part = fs_root.execute(&FileOp::Checksum {
        path: "f.txt".into(),
        offset: 3,
        length: Some(2),
    });
    let empty = fs_root.execute(&FileOp::Checksum {
        path: "f.txt".into(),
        offset: 50,
        length: None,
    });
    assert!(matches!(
        whole,
        FileOpResponse::Checksum {
            bytes: 6,
            total_size: 6,
            ..
        }
    ));
    assert!(matches!(
        part,
        FileOpResponse::Checksum {
            bytes: 2,
            total_size: 6,
            ..
        }
    ));
    assert!(matches!(
        empty,
        FileOpResponse::Checksum {
            bytes: 0,
            total_size: 6,
            ..
        }
    ));
    assert_eq!(
        match whole {
            FileOpResponse::Checksum { sha256, .. } => sha256,
            other => return Err(format!("{other:?}").into()),
        },
        trogon_sandbox_file_ops::content_hash(b"abcdef")?
    );
    fs::create_dir(dir.path().join("d"))?;
    let dir_reply = fs_root.execute(&FileOp::Checksum {
        path: "d".into(),
        offset: 0,
        length: None,
    });
    assert!(matches!(dir_reply, FileOpResponse::Failed { .. }));
    let missing = fs_root.execute(&FileOp::Checksum {
        path: "none".into(),
        offset: 0,
        length: None,
    });
    assert!(matches!(missing, FileOpResponse::NotFound { .. }));
    Ok(())
}

#[test]
fn edit_replaces_when_the_count_matches_and_rejects_otherwise() -> TestResult {
    let (dir, fs_root) = root()?;
    let host = dir.path().join("e.txt");
    fs::write(&host, "one two one")?;
    let rejected = fs_root.execute(&edit_op("e.txt", "one", "1", 1));
    assert_eq!(
        rejected,
        FileOpResponse::EditRejected {
            occurrences: 2,
            expected: 1
        }
    );
    let done = fs_root.execute(&edit_op("e.txt", "one", "1", 2));
    assert_eq!(
        done,
        FileOpResponse::Edited {
            occurrences: 2,
            size: 7
        }
    );
    assert_eq!(fs::read_to_string(&host)?, "1 two 1");
    Ok(())
}

#[test]
fn edit_refuses_bad_specs_and_bad_targets() -> TestResult {
    let (dir, fs_root) = root()?;
    assert!(is_failure(
        &fs_root.execute(&edit_op("e.txt", "", "x", 1)),
        FileOpErrorKind::InvalidRequest
    ));
    assert!(is_failure(
        &fs_root.execute(&edit_op("e.txt", "a", "x", 0)),
        FileOpErrorKind::InvalidRequest
    ));
    assert!(matches!(
        fs_root.execute(&edit_op("missing.txt", "a", "x", 1)),
        FileOpResponse::NotFound { .. }
    ));
    fs::create_dir(dir.path().join("d"))?;
    assert!(is_failure(
        &fs_root.execute(&edit_op("d", "a", "x", 1)),
        FileOpErrorKind::IsADirectory
    ));
    fs::write(dir.path().join("bin"), [0xff, 0xfe, 0xfd])?;
    assert!(is_failure(
        &fs_root.execute(&edit_op("bin", "a", "x", 1)),
        FileOpErrorKind::InvalidRequest
    ));
    let big = fs::File::create(dir.path().join("big"))?;
    big.set_len(FileOp::MAX_READ_BYTES + 1)?;
    assert!(is_failure(
        &fs_root.execute(&edit_op("big", "a", "x", 1)),
        FileOpErrorKind::TooLarge
    ));
    Ok(())
}

#[test]
fn list_walks_in_name_order_and_reports_truncation() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::create_dir_all(dir.path().join("sub/deep"))?;
    fs::write(dir.path().join("b.txt"), b"b")?;
    fs::write(dir.path().join("a.txt"), b"a")?;
    fs::write(dir.path().join("sub/deep/c.txt"), b"c")?;

    let flat = fs_root.execute(&list_op("/workspace", FileRecursion::Flat, None));
    let FileOpResponse::Listing { entries, truncated } = flat else {
        return Err("expected a listing".into());
    };
    let names: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, ["a.txt", "b.txt", "sub"]);
    assert_eq!(truncated, FileResultTruncation::Complete);

    let deep = fs_root.execute(&list_op("", FileRecursion::Recursive, None));
    let FileOpResponse::Listing { entries, .. } = deep else {
        return Err("expected a listing".into());
    };
    let names: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, ["a.txt", "b.txt", "sub", "sub/deep", "sub/deep/c.txt"]);

    let capped = fs_root.execute(&list_op("", FileRecursion::Recursive, Some(1)));
    assert!(matches!(
        capped,
        FileOpResponse::Listing { ref entries, truncated } if entries.len() == 1 && truncated == FileResultTruncation::Truncated
    ));
    let capped_in_subtree = fs_root.execute(&list_op("", FileRecursion::Recursive, Some(4)));
    assert!(matches!(
        capped_in_subtree,
        FileOpResponse::Listing { ref entries, truncated } if entries.len() == 4 && truncated == FileResultTruncation::Truncated
    ));

    let not_dir = fs_root.execute(&list_op("a.txt", FileRecursion::Flat, None));
    assert!(is_failure(&not_dir, FileOpErrorKind::NotADirectory));
    assert!(matches!(
        fs_root.execute(&list_op("nope", FileRecursion::Flat, None)),
        FileOpResponse::NotFound { .. }
    ));
    Ok(())
}

#[test]
fn stat_describes_files_directories_and_the_root() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("f.txt"), b"abc")?;
    fs::create_dir(dir.path().join("d"))?;
    let file = fs_root.execute(&FileOp::Stat {
        path: "/workspace/f.txt".into(),
    });
    let FileOpResponse::Stat { stat } = file else {
        return Err("expected stat".into());
    };
    assert_eq!((stat.kind, stat.size, stat.name.as_str()), (FileKind::File, 3, "f.txt"));
    assert_eq!(stat.path, "/workspace/f.txt");
    let directory = fs_root.execute(&FileOp::Stat { path: "d".into() });
    assert!(matches!(
        directory,
        FileOpResponse::Stat { ref stat } if stat.kind == FileKind::Directory
    ));
    let root_stat = fs_root.execute(&FileOp::Stat {
        path: "/workspace".into(),
    });
    assert!(matches!(root_stat, FileOpResponse::Stat { .. }));
    assert!(matches!(
        fs_root.execute(&FileOp::Stat { path: "zzz".into() }),
        FileOpResponse::NotFound { .. }
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn stat_and_list_report_symlinks_without_following_them() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("target.txt"), b"t")?;
    std::os::unix::fs::symlink("target.txt", dir.path().join("link"))?;
    let reply = fs_root.execute(&FileOp::Stat { path: "link".into() });
    assert!(matches!(
        reply,
        FileOpResponse::Stat { ref stat }
            if stat.kind == FileKind::Symlink && stat.symlink_target.as_deref() == Some("target.txt")
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn stat_reports_other_file_kinds() -> TestResult {
    let (dir, fs_root) = root()?;
    let socket = std::os::unix::net::UnixListener::bind(dir.path().join("sock"))?;
    let reply = fs_root.execute(&FileOp::Stat { path: "sock".into() });
    assert!(matches!(
        reply,
        FileOpResponse::Stat { ref stat } if stat.kind == FileKind::Other
    ));
    drop(socket);
    Ok(())
}

#[test]
fn paths_cannot_leave_the_root() -> TestResult {
    let (_dir, fs_root) = root()?;
    for path in ["../etc/passwd", "/etc/passwd", "/workspacefoo/x", "a/../../b"] {
        let reply = fs_root.execute(&read_op(path, 0, None, None));
        assert!(is_failure(&reply, FileOpErrorKind::InvalidPath), "{path}: {reply:?}");
    }
    let reply = fs_root.execute(&read_op("a\0b", 0, None, None));
    assert!(is_failure(&reply, FileOpErrorKind::InvalidPath), "{reply:?}");
    Ok(())
}

#[test]
fn dot_segments_and_a_missing_prefix_resolve_inside_the_root() -> TestResult {
    let dir = TempDir::new()?;
    let bare = FsRoot::new(dir.path(), None)?;
    bare.execute(&write_op("/a/./b.txt", b"x")?);
    assert_eq!(fs::read(dir.path().join("a/b.txt"))?, b"x");
    let reply = bare.execute(&FileOp::Stat {
        path: "a/../a/b.txt".into(),
    });
    assert!(matches!(reply, FileOpResponse::Stat { .. }));
    assert!(matches!(
        FsRoot::new(dir.path().join("none"), None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlinks_that_point_outside_the_root_are_refused() -> TestResult {
    let (dir, fs_root) = root()?;
    let outside = TempDir::new()?;
    fs::write(outside.path().join("secret"), b"secret")?;
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape"))?;
    std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("direct"))?;
    for path in ["escape/secret", "direct"] {
        let reply = fs_root.execute(&read_op(path, 0, None, None));
        assert!(is_failure(&reply, FileOpErrorKind::InvalidPath), "{path}: {reply:?}");
    }
    let write = fs_root.execute(&write_op("escape/new", b"x")?);
    assert!(is_failure(&write, FileOpErrorKind::InvalidPath), "{write:?}");
    assert!(!outside.path().join("new").exists());
    Ok(())
}

#[test]
fn remove_deletes_files_and_trees() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::create_dir_all(dir.path().join("tree/sub"))?;
    fs::write(dir.path().join("tree/sub/f"), b"1")?;
    fs::write(dir.path().join("file"), b"1")?;
    let remove = |path: &str, recursive| FileOp::Remove {
        path: path.into(),
        recursive,
    };
    assert_eq!(
        fs_root.execute(&remove("file", FileRecursion::Flat)),
        FileOpResponse::Removed {
            existed: FilePresence::Present
        }
    );
    let refused = fs_root.execute(&remove("tree", FileRecursion::Flat));
    assert!(matches!(refused, FileOpResponse::Failed { .. }), "{refused:?}");
    assert_eq!(
        fs_root.execute(&remove("tree", FileRecursion::Recursive)),
        FileOpResponse::Removed {
            existed: FilePresence::Present
        }
    );
    assert!(!dir.path().join("tree").exists());
    assert_eq!(
        fs_root.execute(&remove("tree", FileRecursion::Recursive)),
        FileOpResponse::Removed {
            existed: FilePresence::Absent
        }
    );
    fs::create_dir(dir.path().join("empty"))?;
    assert_eq!(
        fs_root.execute(&remove("empty", FileRecursion::Flat)),
        FileOpResponse::Removed {
            existed: FilePresence::Present
        }
    );
    let root_reply = fs_root.execute(&remove("/workspace", FileRecursion::Recursive));
    assert!(is_failure(&root_reply, FileOpErrorKind::InvalidPath));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_recursive_remove_deletes_links_without_following_them() -> TestResult {
    let (dir, fs_root) = root()?;
    let outside = TempDir::new()?;
    fs::write(outside.path().join("keep"), b"keep")?;
    fs::create_dir(dir.path().join("tree"))?;
    std::os::unix::fs::symlink(outside.path(), dir.path().join("tree/link"))?;
    fs::create_dir(dir.path().join("tree/inner"))?;
    fs::write(dir.path().join("tree/inner/f"), b"1")?;
    let reply = fs_root.execute(&FileOp::Remove {
        path: "tree".into(),
        recursive: FileRecursion::Recursive,
    });
    assert_eq!(
        reply,
        FileOpResponse::Removed {
            existed: FilePresence::Present
        }
    );
    assert!(outside.path().join("keep").exists());
    Ok(())
}

#[test]
fn mkdir_creates_with_and_without_parents() -> TestResult {
    let (dir, fs_root) = root()?;
    let mkdir = |path: &str, parents| FileOp::Mkdir {
        path: path.into(),
        parents,
    };
    assert_eq!(
        fs_root.execute(&mkdir("a/b/c", FileParentCreation::Create)),
        FileOpResponse::Created {
            already_existed: FilePresence::Absent
        }
    );
    assert!(dir.path().join("a/b/c").is_dir());
    assert_eq!(
        fs_root.execute(&mkdir("a/b/c", FileParentCreation::Create)),
        FileOpResponse::Created {
            already_existed: FilePresence::Present
        }
    );
    assert!(matches!(
        fs_root.execute(&mkdir("x/y", FileParentCreation::Require)),
        FileOpResponse::NotFound { .. }
    ));
    assert_eq!(
        fs_root.execute(&mkdir("one", FileParentCreation::Require)),
        FileOpResponse::Created {
            already_existed: FilePresence::Absent
        }
    );
    assert_eq!(
        fs_root.execute(&mkdir("/workspace", FileParentCreation::Create)),
        FileOpResponse::Created {
            already_existed: FilePresence::Present
        }
    );
    fs::write(dir.path().join("file"), b"1")?;
    assert!(is_failure(
        &fs_root.execute(&mkdir("file", FileParentCreation::Create)),
        FileOpErrorKind::AlreadyExists
    ));
    assert!(matches!(
        fs_root.execute(&mkdir("file/inside", FileParentCreation::Create)),
        FileOpResponse::Failed { .. }
    ));
    Ok(())
}

#[test]
fn rename_moves_and_honours_the_overwrite_choice() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("a"), b"a")?;
    fs::write(dir.path().join("b"), b"b")?;
    let rename = |from: &str, to: &str, overwrite| FileOp::Rename {
        from: from.into(),
        to: to.into(),
        overwrite,
    };
    let refused = fs_root.execute(&rename("a", "b", FileOverwrite::Refuse));
    assert!(is_failure(&refused, FileOpErrorKind::AlreadyExists), "{refused:?}");
    assert_eq!(
        fs_root.execute(&rename("a", "b", FileOverwrite::Overwrite)),
        FileOpResponse::Renamed
    );
    assert_eq!(fs::read(dir.path().join("b"))?, b"a");
    assert_eq!(
        fs_root.execute(&rename("b", "c", FileOverwrite::Refuse)),
        FileOpResponse::Renamed
    );
    assert!(matches!(
        fs_root.execute(&rename("gone", "d", FileOverwrite::Refuse)),
        FileOpResponse::NotFound { .. }
    ));
    assert!(matches!(
        fs_root.execute(&rename("c", "nodir/d", FileOverwrite::Refuse)),
        FileOpResponse::NotFound { .. }
    ));
    assert!(is_failure(
        &fs_root.execute(&rename("/workspace", "d", FileOverwrite::Refuse)),
        FileOpErrorKind::InvalidPath
    ));
    assert!(is_failure(
        &fs_root.execute(&rename("c", "/workspace", FileOverwrite::Refuse)),
        FileOpErrorKind::InvalidPath
    ));
    Ok(())
}

#[test]
fn search_lists_paths_by_glob_and_lines_by_pattern() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::create_dir_all(dir.path().join("src/inner"))?;
    fs::write(dir.path().join("src/a.rs"), "fn main() {}\nlet Needle = 1;\n")?;
    fs::write(dir.path().join("src/inner/b.rs"), "needle here\n")?;
    fs::write(dir.path().join("notes.md"), "needle in notes\n")?;
    fs::write(dir.path().join("blob.bin"), b"needle\0binary")?;

    let by_glob = fs_root.execute(&search_op("", Some("*.rs"), None));
    let FileOpResponse::SearchResults { matches, .. } = by_glob else {
        return Err("expected results".into());
    };
    let paths: Vec<_> = matches.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "src/inner/b.rs"]);

    let nested = fs_root.execute(&search_op("", Some("src/**/*.rs"), None));
    assert!(matches!(nested, FileOpResponse::SearchResults { ref matches, .. } if matches.len() == 2));

    let by_text = fs_root.execute(&search_op("", None, Some("needle")));
    let FileOpResponse::SearchResults {
        matches, files_scanned, ..
    } = by_text
    else {
        return Err("expected results".into());
    };
    assert_eq!(files_scanned, 4);
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].line, Some(1));

    let folded = fs_root.execute(&FileOp::Search {
        path: "".into(),
        glob: Some("*.rs".into()),
        pattern: Some("NEEDLE".into()),
        regex: FileSearchSyntax::Literal,
        case_insensitive: FileCaseSensitivity::Insensitive,
        max_results: None,
        max_file_bytes: None,
    });
    assert!(matches!(folded, FileOpResponse::SearchResults { ref matches, .. } if matches.len() == 2));

    let regex = fs_root.execute(&FileOp::Search {
        path: "src".into(),
        glob: None,
        pattern: Some(r"fn\s+\w+".into()),
        regex: FileSearchSyntax::Regex,
        case_insensitive: FileCaseSensitivity::Sensitive,
        max_results: None,
        max_file_bytes: None,
    });
    assert!(matches!(regex, FileOpResponse::SearchResults { ref matches, .. } if matches.len() == 1));

    let single = fs_root.execute(&search_op("notes.md", None, Some("needle")));
    assert!(matches!(single, FileOpResponse::SearchResults { ref matches, .. } if matches.len() == 1));
    Ok(())
}

#[test]
fn search_limits_results_and_file_sizes() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("a.txt"), "x\nx\nx\n")?;
    fs::write(dir.path().join("b.txt"), "x\n")?;
    fs::write(dir.path().join("long.txt"), format!("{}\n", "y".repeat(900)))?;
    let capped_lines = fs_root.execute(&FileOp::Search {
        path: "".into(),
        glob: None,
        pattern: Some("x".into()),
        regex: FileSearchSyntax::Literal,
        case_insensitive: FileCaseSensitivity::Sensitive,
        max_results: Some(2),
        max_file_bytes: None,
    });
    assert!(matches!(
        capped_lines,
        FileOpResponse::SearchResults { ref matches, truncated, .. }
            if matches.len() == 2 && truncated == FileResultTruncation::Truncated
    ));
    let capped_paths = fs_root.execute(&FileOp::Search {
        path: "".into(),
        glob: Some("*.txt".into()),
        pattern: None,
        regex: FileSearchSyntax::Literal,
        case_insensitive: FileCaseSensitivity::Sensitive,
        max_results: Some(1),
        max_file_bytes: None,
    });
    assert!(matches!(
        capped_paths,
        FileOpResponse::SearchResults { ref matches, truncated, .. }
            if matches.len() == 1 && truncated == FileResultTruncation::Truncated
    ));
    let skipped = fs_root.execute(&FileOp::Search {
        path: "".into(),
        glob: None,
        pattern: Some("x".into()),
        regex: FileSearchSyntax::Literal,
        case_insensitive: FileCaseSensitivity::Sensitive,
        max_results: None,
        max_file_bytes: Some(3),
    });
    assert!(matches!(skipped, FileOpResponse::SearchResults { ref matches, .. } if matches.len() == 1));
    let long = fs_root.execute(&search_op("long.txt", None, Some("y")));
    let FileOpResponse::SearchResults { matches, .. } = long else {
        return Err("expected results".into());
    };
    assert_eq!(matches[0].text.as_deref().map(str::len), Some(500));
    Ok(())
}

#[test]
fn search_rejects_bad_patterns_and_missing_paths() -> TestResult {
    let (_dir, fs_root) = root()?;
    let bad_regex = fs_root.execute(&FileOp::Search {
        path: "".into(),
        glob: None,
        pattern: Some("(".into()),
        regex: FileSearchSyntax::Regex,
        case_insensitive: FileCaseSensitivity::Sensitive,
        max_results: None,
        max_file_bytes: None,
    });
    assert!(is_failure(&bad_regex, FileOpErrorKind::InvalidRequest), "{bad_regex:?}");
    let huge_glob = "?".repeat(200_000);
    let bad_glob = fs_root.execute(&search_op("", Some(&huge_glob), None));
    assert!(is_failure(&bad_glob, FileOpErrorKind::InvalidRequest), "{bad_glob:?}");
    assert!(matches!(
        fs_root.execute(&search_op("nope", None, Some("x"))),
        FileOpResponse::NotFound { .. }
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn search_skips_unreadable_files_and_symlinks() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("ok.txt"), "x\n")?;
    fs::write(dir.path().join("locked.txt"), "x\n")?;
    fs::set_permissions(dir.path().join("locked.txt"), fs::Permissions::from_mode(0o000))?;
    std::os::unix::fs::symlink("ok.txt", dir.path().join("link.txt"))?;
    let reply = fs_root.execute(&search_op("", None, Some("x")));
    fs::set_permissions(dir.path().join("locked.txt"), fs::Permissions::from_mode(0o644))?;
    let FileOpResponse::SearchResults { matches, .. } = reply else {
        return Err("expected results".into());
    };
    assert!(matches.iter().all(|m| m.path != "link.txt"));
    Ok(())
}

#[test]
fn archives_round_trip_a_tree() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::create_dir_all(dir.path().join("src/sub"))?;
    fs::write(dir.path().join("src/a.txt"), b"alpha")?;
    fs::write(dir.path().join("src/sub/b.txt"), b"beta")?;
    let FileOpResponse::Archive { data, entries, bytes } = fs_root.execute(&FileOp::ArchiveGet {
        path: "src".into(),
        max_bytes: None,
    }) else {
        return Err("expected an archive".into());
    };
    assert_eq!((entries, bytes), (3, 9));
    let put = fs_root.execute(&FileOp::ArchivePut {
        path: "copy".into(),
        data,
    });
    assert_eq!(put, FileOpResponse::Extracted { entries: 3, bytes: 9 });
    assert_eq!(fs::read(dir.path().join("copy/sub/b.txt"))?, b"beta");
    Ok(())
}

#[test]
fn an_archive_of_one_file_and_the_root_destination_work() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("one.txt"), b"1")?;
    let FileOpResponse::Archive { data, entries, .. } = fs_root.execute(&FileOp::ArchiveGet {
        path: "one.txt".into(),
        max_bytes: None,
    }) else {
        return Err("expected an archive".into());
    };
    assert_eq!(entries, 1);
    fs::remove_file(dir.path().join("one.txt"))?;
    let put = fs_root.execute(&FileOp::ArchivePut { path: "".into(), data });
    assert_eq!(put, FileOpResponse::Extracted { entries: 1, bytes: 1 });
    assert_eq!(fs::read(dir.path().join("one.txt"))?, b"1");
    Ok(())
}

#[test]
fn archive_get_enforces_the_byte_cap_and_the_entry_cap() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("f.txt"), vec![b'z'; 100])?;
    let capped = fs_root.execute(&FileOp::ArchiveGet {
        path: "f.txt".into(),
        max_bytes: Some(10),
    });
    assert!(is_failure(&capped, FileOpErrorKind::TooLarge), "{capped:?}");
    fs::create_dir(dir.path().join("d"))?;
    fs::write(dir.path().join("d/f.txt"), vec![b'z'; 100])?;
    let capped_dir = fs_root.execute(&FileOp::ArchiveGet {
        path: "d".into(),
        max_bytes: Some(10),
    });
    assert!(is_failure(&capped_dir, FileOpErrorKind::TooLarge), "{capped_dir:?}");
    assert!(matches!(
        fs_root.execute(&FileOp::ArchiveGet {
            path: "none".into(),
            max_bytes: None
        }),
        FileOpResponse::NotFound { .. }
    ));
    Ok(())
}

#[test]
fn archive_put_refuses_hostile_archives() -> TestResult {
    let (dir, fs_root) = root()?;
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(1);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    let name = b"../escape.txt";
    header.as_mut_bytes()[..name.len()].copy_from_slice(name);
    header.set_cksum();
    builder.append(&header, &b"x"[..])?;
    let hostile = builder.into_inner()?;
    let reply = fs_root.execute(&FileOp::ArchivePut {
        path: "".into(),
        data: data(&hostile)?,
    });
    assert!(is_failure(&reply, FileOpErrorKind::InvalidRequest), "{reply:?}");
    assert!(!dir.path().parent().ok_or("no parent")?.join("escape.txt").exists());

    let garbage = fs_root.execute(&FileOp::ArchivePut {
        path: "".into(),
        data: data(&[7u8; 600])?,
    });
    assert!(matches!(garbage, FileOpResponse::Failed { .. }), "{garbage:?}");
    Ok(())
}

#[test]
fn archive_put_applies_entry_modes_and_skips_the_root_entry() -> TestResult {
    let (dir, fs_root) = root()?;
    let mut builder = tar::Builder::new(Vec::new());
    let mut top = tar::Header::new_gnu();
    top.set_entry_type(tar::EntryType::Directory);
    top.set_size(0);
    top.set_mode(0o755);
    builder.append_data(&mut top, "./", std::io::empty())?;
    let mut dir_header = tar::Header::new_gnu();
    dir_header.set_entry_type(tar::EntryType::Directory);
    dir_header.set_size(0);
    dir_header.set_mode(0o755);
    builder.append_data(&mut dir_header, "pkg/", std::io::empty())?;
    let mut file_header = tar::Header::new_gnu();
    file_header.set_size(3);
    file_header.set_mode(0o700);
    builder.append_data(&mut file_header, "pkg/run.sh", &b"abc"[..])?;
    let archive = builder.into_inner()?;
    let reply = fs_root.execute(&FileOp::ArchivePut {
        path: "/workspace".into(),
        data: data(&archive)?,
    });
    assert_eq!(reply, FileOpResponse::Extracted { entries: 2, bytes: 3 });
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(dir.path().join("pkg/run.sh"))?.permissions().mode() & 0o777,
            0o700
        );
    }
    Ok(())
}

#[test]
fn tracking_verbs_are_not_served_by_the_directory_root() -> TestResult {
    let (_dir, fs_root) = root()?;
    let reply = fs_root.execute(&FileOp::Checkpoint {
        path: "".into(),
        hash: FileContentHashing::Metadata,
    });
    assert!(matches!(reply, FileOpResponse::Unsupported { ref message } if message.contains("checkpoint")));
    let typed = fs_root.try_execute(&FileOp::WatchStop { watch_id: "w".into() });
    assert!(matches!(typed, Err(FileOpFailure::Unsupported { .. })));
    for op in [
        FileOp::Changes {
            since: None,
            path: None,
        },
        FileOp::WatchStart {
            path: "".into(),
            recursive: FileRecursion::Flat,
        },
        FileOp::WatchPoll {
            watch_id: "w".into(),
            after_seq: 0,
            wait_ms: None,
        },
    ] {
        assert!(matches!(fs_root.execute(&op), FileOpResponse::Unsupported { .. }));
    }
    Ok(())
}

#[test]
fn manifest_fingerprints_files_and_trees() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::create_dir_all(dir.path().join("d/sub"))?;
    fs::write(dir.path().join("d/a.txt"), b"abc")?;
    fs::write(dir.path().join("d/sub/b.txt"), b"de")?;
    let flat = fs_root.manifest("/workspace/d", FileRecursion::Flat, FileContentHashing::Metadata)?;
    let paths: Vec<_> = flat.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths, ["/workspace/d/a.txt", "/workspace/d/sub"]);
    let deep = fs_root.manifest("d", FileRecursion::Recursive, FileContentHashing::Content)?;
    assert_eq!(deep.len(), 3);
    assert!(
        deep.get("/workspace/d/a.txt")
            .is_some_and(|entry| entry.hash().is_some() && entry.size() == 3)
    );
    let file = fs_root.manifest("d/a.txt", FileRecursion::Flat, FileContentHashing::Metadata)?;
    assert_eq!(file.len(), 1);
    let whole = fs_root.manifest("", FileRecursion::Flat, FileContentHashing::Metadata)?;
    assert!(whole.contains_path("/workspace/d"));
    assert!(matches!(
        fs_root.manifest("none", FileRecursion::Flat, FileContentHashing::Metadata),
        Err(FileOpFailure::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn manifest_without_a_prefix_uses_root_relative_paths() -> TestResult {
    let dir = TempDir::new()?;
    fs::create_dir(dir.path().join("d"))?;
    fs::write(dir.path().join("d/a.txt"), b"abc")?;
    let bare = FsRoot::new(dir.path(), None)?;
    let manifest = bare.manifest("", FileRecursion::Recursive, FileContentHashing::Metadata)?;
    assert!(manifest.contains_path("/d/a.txt"));
    let under = bare.manifest("d", FileRecursion::Flat, FileContentHashing::Metadata)?;
    assert!(under.contains_path("/d/a.txt"));
    Ok(())
}

#[test]
fn manifest_is_capped_at_the_tracking_limit() -> TestResult {
    let (dir, fs_root) = root()?;
    let tracked = FileOp::TRACKING_MAX_ENTRIES;
    for index in 0..=tracked {
        fs::File::create(dir.path().join(format!("f{index}")))?;
    }
    let result = fs_root.manifest("", FileRecursion::Flat, FileContentHashing::Metadata);
    assert!(matches!(
        result,
        Err(FileOpFailure::Failed {
            kind: FileOpErrorKind::TooLarge,
            ..
        })
    ));
    Ok(())
}

#[test]
fn remove_reports_paths_that_cannot_exist() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("plain"), b"x")?;
    let remove = |path: &str| FileOp::Remove {
        path: path.into(),
        recursive: FileRecursion::Flat,
    };
    let missing_parent = fs_root.execute(&remove("nope/child"));
    assert!(
        !matches!(missing_parent, FileOpResponse::Failed { .. }),
        "{missing_parent:?}"
    );
    let under_a_file = fs_root.execute(&remove("plain/child"));
    assert!(
        matches!(under_a_file, FileOpResponse::Failed { .. }),
        "{under_a_file:?}"
    );
    let too_long = fs_root.execute(&remove(&"a".repeat(300)));
    assert!(matches!(too_long, FileOpResponse::Failed { .. }), "{too_long:?}");
    Ok(())
}

#[test]
fn a_failed_write_to_a_new_file_leaves_nothing_behind() -> TestResult {
    let (dir, fs_root) = root()?;
    let reply = fs_root.execute(&FileOp::Write {
        path: "new.txt".into(),
        data: data(b"x")?,
        offset: u64::MAX,
        truncate: FileWriteTruncation::Keep,
        mode: None,
        create_parents: FileParentCreation::Create,
    });
    assert!(matches!(reply, FileOpResponse::Failed { .. }), "{reply:?}");
    assert!(!dir.path().join("new.txt").exists());
    Ok(())
}

#[test]
fn a_whole_write_below_a_file_fails() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::write(dir.path().join("plain"), b"x")?;
    let reply = fs_root.execute(&FileOp::Write {
        path: "plain/child".into(),
        data: data(b"y")?,
        offset: 0,
        truncate: FileWriteTruncation::Truncate,
        mode: None,
        create_parents: FileParentCreation::Require,
    });
    assert!(matches!(reply, FileOpResponse::Failed { .. }), "{reply:?}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn archive_get_skips_links_and_refuses_an_archive_over_the_limit() -> TestResult {
    let (dir, fs_root) = root()?;
    fs::create_dir(dir.path().join("tree"))?;
    fs::write(dir.path().join("tree/real"), b"abc")?;
    std::os::unix::fs::symlink("real", dir.path().join("tree/link"))?;
    let FileOpResponse::Archive { entries, .. } = fs_root.execute(&FileOp::ArchiveGet {
        path: "tree".into(),
        max_bytes: None,
    }) else {
        return Err("expected an archive".into());
    };
    assert_eq!(entries, 1);

    fs::create_dir(dir.path().join("huge"))?;
    // Real bytes, not a hole, so the archive holds the whole length on every platform.
    fs::write(
        dir.path().join("huge/blob"),
        vec![1u8; FileOp::MAX_ARCHIVE_BYTES as usize],
    )?;
    let reply = fs_root.execute(&FileOp::ArchiveGet {
        path: "huge".into(),
        max_bytes: None,
    });
    assert!(is_failure(&reply, FileOpErrorKind::TooLarge), "{reply:?}");
    Ok(())
}

#[test]
fn search_stops_after_the_scan_byte_budget() -> TestResult {
    let (dir, fs_root) = root()?;
    for name in ["a.bin", "b.bin"] {
        fs::File::create(dir.path().join(name))?.set_len(129 * 1024 * 1024)?;
    }
    let reply = fs_root.execute(&FileOp::Search {
        path: "".into(),
        glob: None,
        pattern: Some("needle".into()),
        regex: FileSearchSyntax::Literal,
        case_insensitive: FileCaseSensitivity::Sensitive,
        max_results: None,
        max_file_bytes: Some(u64::MAX),
    });
    let FileOpResponse::SearchResults {
        truncated,
        files_scanned,
        ..
    } = reply
    else {
        return Err(format!("expected search results, got {reply:?}").into());
    };
    assert_eq!(truncated, FileResultTruncation::Truncated);
    assert_eq!(files_scanned, 1);
    Ok(())
}
