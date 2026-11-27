use std::error::Error;
use std::io::{self, Read as _};

use trogon_sandbox_contract::{
    Base64Data, FileOpErrorKind, FileOpResponse, FileSha256, FileWatchEventKind, FileWatchTarget,
};
use trogon_sandbox_file_ops::{
    ArchiveSummary, ChangeHint, EditOutcome, EditSpec, EditSpecError, FileOpError, GlobPattern, Manifest, ManifestDiff,
    ManifestEntry, SandboxPrefix, SandboxPrefixError, StreamHasher, WatchOverflow, content_hash,
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn sandbox_prefixes_are_parsed() -> TestResult {
    assert_eq!(SandboxPrefix::new("/workspace/")?.as_str(), "/workspace");
    assert_eq!("/a/b".parse::<SandboxPrefix>()?.to_string(), "/a/b");
    assert_eq!(SandboxPrefix::new("/")?.as_str(), "");
    assert_eq!(SandboxPrefix::new("rel"), Err(SandboxPrefixError::NotAbsolute));
    assert_eq!(SandboxPrefix::new("/a\0b"), Err(SandboxPrefixError::ContainsNul));
    for bad in ["/a/../b", "/a//b", "/a/./b"] {
        assert_eq!(SandboxPrefix::new(bad), Err(SandboxPrefixError::NotNormalized), "{bad}");
    }
    assert!(!SandboxPrefixError::NotAbsolute.to_string().is_empty());
    Ok(())
}

#[test]
fn edit_specs_are_parsed_and_applied() -> TestResult {
    assert_eq!(EditSpec::new("", "x", 1), Err(EditSpecError::EmptyFind));
    assert_eq!(EditSpec::new("a", "x", 0), Err(EditSpecError::ZeroExpectedOccurrences));
    let spec = EditSpec::new("a", "b", 2)?;
    assert_eq!(
        (spec.find(), spec.replace(), spec.expected_occurrences().get()),
        ("a", "b", 2)
    );
    assert_eq!(
        spec.apply(b"a-a")?,
        EditOutcome::Replaced {
            content: b"b-b".to_vec(),
            occurrences: 2
        }
    );
    assert_eq!(spec.apply(b"a")?, EditOutcome::Rejected { occurrences: 1 });
    assert!(matches!(
        spec.apply(&[0xff, 0xfe]),
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::InvalidRequest,
            ..
        })
    ));
    let failure = FileOpError::from(EditSpecError::EmptyFind);
    assert!(matches!(failure, FileOpError::Failed { .. }));
    assert!(!EditSpecError::ZeroExpectedOccurrences.to_string().is_empty());
    Ok(())
}

#[test]
fn globs_match_like_shell_patterns() -> TestResult {
    let star = GlobPattern::new("*.rs")?;
    assert!(star.is_match("main.rs"));
    assert!(!star.is_match("src/main.rs"));
    let deep = GlobPattern::new("src/**/*.rs")?;
    assert!(deep.is_match("src/a/b/c.rs"));
    assert!(deep.is_match("src/c.rs"));
    let any = GlobPattern::new("docs/**")?;
    assert!(any.is_match("docs/a/b"));
    let one = GlobPattern::new("a?c.txt")?;
    assert!(one.is_match("abc.txt"));
    assert!(!one.is_match("a/c.txt"));
    let literal = GlobPattern::new("a.b(c)")?;
    assert!(literal.is_match("a.b(c)"));
    let error = GlobPattern::new(&"?".repeat(200_000))
        .err()
        .ok_or("expected an error")?;
    assert!(std::error::Error::source(&error).is_some());
    Ok(())
}

#[test]
fn hashes_agree_between_one_shot_and_streaming() -> TestResult {
    let mut hasher = StreamHasher::new();
    hasher.update(b"hello ");
    hasher.update(b"world");
    assert_eq!(hasher.finish()?, content_hash(b"hello world")?);
    assert_eq!(
        content_hash(b"")?.as_str(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    Ok(())
}

fn tar_with(entries: &[(&str, tar::EntryType, &[u8])]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, kind, body) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(*kind);
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        if kind.is_symlink() {
            header.set_link_name("target")?;
        }
        builder.append_data(&mut header, name, *body)?;
    }
    Ok(builder.into_inner()?)
}

#[test]
fn archive_summaries_count_entries_and_bytes() -> TestResult {
    let good = tar_with(&[
        ("d/", tar::EntryType::Directory, b""),
        ("d/f.txt", tar::EntryType::Regular, b"abc"),
    ])?;
    let summary = ArchiveSummary::validate(&good)?;
    assert_eq!((summary.entries(), summary.bytes()), (2, 3));
    let link = tar_with(&[("l", tar::EntryType::Symlink, b"")])?;
    assert!(matches!(
        ArchiveSummary::validate(&link),
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::InvalidRequest,
            ..
        })
    ));
    assert!(matches!(
        ArchiveSummary::validate(&[9u8; 700]),
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::InvalidRequest,
            ..
        })
    ));
    let oversized = vec![0u8; 64 * 1024 * 1024 + 1];
    assert!(matches!(
        ArchiveSummary::validate(&oversized),
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::TooLarge,
            ..
        })
    ));
    Ok(())
}

#[test]
fn archive_summaries_refuse_absolute_and_oversized_content() -> TestResult {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_mode(0o644);
    let name = b"/abs.txt";
    header.as_mut_bytes()[..name.len()].copy_from_slice(name);
    header.set_cksum();
    builder.append(&header, io::empty())?;
    let absolute = builder.into_inner()?;
    assert!(matches!(
        ArchiveSummary::validate(&absolute),
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::InvalidRequest,
            ..
        })
    ));

    let mut sparse = tar::Builder::new(Vec::new());
    for index in 0..2 {
        let mut header = tar::Header::new_gnu();
        header.set_size(40 * 1024 * 1024);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        let name = format!("big{index}");
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_cksum();
        sparse.append(&header, io::repeat(0).take(40 * 1024 * 1024))?;
    }
    let big = sparse.into_inner()?;
    assert!(matches!(
        ArchiveSummary::validate(&big),
        Err(FileOpError::Failed {
            kind: FileOpErrorKind::TooLarge,
            ..
        })
    ));
    Ok(())
}

#[test]
fn failures_map_onto_replies() -> TestResult {
    let cases = [
        (io::ErrorKind::PermissionDenied, FileOpErrorKind::PermissionDenied),
        (io::ErrorKind::AlreadyExists, FileOpErrorKind::AlreadyExists),
        (io::ErrorKind::NotADirectory, FileOpErrorKind::NotADirectory),
        (io::ErrorKind::IsADirectory, FileOpErrorKind::IsADirectory),
        (io::ErrorKind::DirectoryNotEmpty, FileOpErrorKind::DirectoryNotEmpty),
        (io::ErrorKind::InvalidInput, FileOpErrorKind::InvalidPath),
        (io::ErrorKind::TimedOut, FileOpErrorKind::Io),
    ];
    for (io_kind, wanted) in cases {
        let failure = FileOpError::from_io(io::Error::from(io_kind), "p");
        assert!(
            matches!(&failure, FileOpError::Failed { kind, .. } if *kind == wanted),
            "{io_kind:?}"
        );
        assert!(!failure.to_string().is_empty());
    }
    let missing = FileOpError::from_io(io::Error::from(io::ErrorKind::NotFound), "gone");
    assert_eq!(missing.to_string(), "gone: not found");
    assert_eq!(
        FileOpResponse::from(missing),
        FileOpResponse::NotFound { path: "gone".into() }
    );
    let unsupported = FileOpError::Unsupported { message: "no".into() };
    assert_eq!(unsupported.to_string(), "no");
    assert_eq!(
        FileOpResponse::from(unsupported),
        FileOpResponse::Unsupported { message: "no".into() }
    );
    let failed = FileOpError::invalid_path("bad");
    assert_eq!(
        FileOpResponse::from(failed),
        FileOpResponse::Failed {
            kind: FileOpErrorKind::InvalidPath,
            message: "bad".into()
        }
    );
    let digest = FileSha256::new("x").err().ok_or("expected an error")?;
    assert!(matches!(
        FileOpError::from(digest),
        FileOpError::Failed {
            kind: FileOpErrorKind::Io,
            ..
        }
    ));
    let base64 = Base64Data::new("!!").err().ok_or("expected an error")?;
    assert!(matches!(
        FileOpError::from(base64),
        FileOpError::Failed {
            kind: FileOpErrorKind::Io,
            ..
        }
    ));
    Ok(())
}

#[test]
fn manifests_diff_by_size_stamp_hash_and_kind() -> TestResult {
    let hash_a = content_hash(b"a")?;
    let hash_b = content_hash(b"b")?;
    let stamp = |value: &str| Some(value.to_string());
    let before: Manifest = [
        ("/keep".to_string(), ManifestEntry::file(1, stamp("1"), None)),
        ("/gone".to_string(), ManifestEntry::file(1, stamp("1"), None)),
        ("/size".to_string(), ManifestEntry::file(1, stamp("1"), None)),
        ("/stamp".to_string(), ManifestEntry::file(1, stamp("1"), None)),
        (
            "/hash".to_string(),
            ManifestEntry::file(1, stamp("1"), Some(hash_a.clone())),
        ),
        (
            "/hash-same".to_string(),
            ManifestEntry::file(1, stamp("1"), Some(hash_a.clone())),
        ),
        ("/kind".to_string(), ManifestEntry::file(1, stamp("1"), None)),
        ("/dir".to_string(), ManifestEntry::directory()),
    ]
    .into_iter()
    .collect();
    let mut after = Manifest::new();
    assert!(after.is_empty());
    after.insert("/keep", ManifestEntry::file(1, stamp("1"), None));
    after.insert("/new", ManifestEntry::file(1, stamp("1"), None));
    after.insert("/size", ManifestEntry::file(2, stamp("1"), None));
    after.insert("/stamp", ManifestEntry::file(1, stamp("2"), None));
    after.insert("/hash", ManifestEntry::file(1, stamp("1"), Some(hash_b)));
    after.insert("/hash-same", ManifestEntry::file(1, stamp("9"), Some(hash_a)));
    after.insert("/kind", ManifestEntry::directory());
    after.insert("/dir", ManifestEntry::directory());
    let diff = before.diff(&after);
    assert_eq!(diff.added, ["/new"]);
    assert_eq!(diff.deleted, ["/gone"]);
    assert_eq!(diff.modified, ["/hash", "/kind", "/size", "/stamp"]);
    let walked: Vec<_> = (&after).into_iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(walked.len(), after.len());
    assert!(after.iter().count() == 8);
    assert!(after.insert("/keep", ManifestEntry::directory()).is_some());
    Ok(())
}

#[test]
fn manifest_entries_expose_their_fingerprint() -> TestResult {
    let hash = content_hash(b"x")?;
    let file = ManifestEntry::file(7, Some("1.5".into()), Some(hash.clone()));
    assert_eq!((file.size(), file.stamp(), file.hash()), (7, Some("1.5"), Some(&hash)));
    assert_eq!(file.target(), FileWatchTarget::File);
    assert!(!file.is_directory());
    let directory = ManifestEntry::directory();
    assert!(directory.is_directory());
    assert_eq!((directory.size(), directory.stamp(), directory.hash()), (0, None, None));
    Ok(())
}

#[test]
fn stat_output_is_parsed_line_by_line() {
    let manifest = Manifest::from_stat_output(
        "regular file|12|1700000000.5|/w/a.txt\ndirectory|4096|1700000000|/w/d\nbroken line\nregular file|x|9|/w/b\n",
    );
    assert_eq!(manifest.len(), 3);
    assert_eq!(manifest.get("/w/a.txt").map(ManifestEntry::size), Some(12));
    assert!(manifest.get("/w/d").is_some_and(ManifestEntry::is_directory));
    assert_eq!(manifest.get("/w/b").map(ManifestEntry::size), Some(0));
}

#[test]
fn diffs_can_be_restricted_to_a_subtree() {
    let mut diff = ManifestDiff {
        added: vec!["/w/a/x".into(), "/w/ab/y".into(), "/w/a".into()],
        modified: vec!["/w/b".into()],
        deleted: vec!["/w/a/z".into()],
    };
    diff.restrict("/w/a/");
    assert_eq!(diff.added, ["/w/a/x", "/w/a"]);
    assert!(diff.modified.is_empty());
    assert_eq!(diff.deleted, ["/w/a/z"]);
    diff.restrict("");
    assert_eq!(diff.added.len(), 2);
}

#[test]
fn hints_and_overflows_are_plain_values() {
    let hint = ChangeHint {
        path: "/w/a".into(),
        kind: FileWatchEventKind::Created,
        target: FileWatchTarget::File,
    };
    assert_eq!(hint.clone(), hint);
    let overflow = WatchOverflow {
        watch_id: "w-1".into(),
        dropped_through: 4,
    };
    assert_eq!(overflow.clone(), overflow);
}

#[test]
fn an_archive_declaring_more_bytes_than_the_limit_is_refused() -> TestResult {
    let mut header = tar::Header::new_gnu();
    header.set_size(trogon_sandbox_contract::FileOp::MAX_ARCHIVE_BYTES + 1);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    let name = b"big";
    header.as_mut_bytes()[..name.len()].copy_from_slice(name);
    header.set_cksum();
    assert!(ArchiveSummary::validate(header.as_bytes()).is_err());
    Ok(())
}

#[test]
fn an_archive_with_too_many_entries_is_refused() -> TestResult {
    let mut builder = tar::Builder::new(Vec::new());
    for index in 0..100_001u32 {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o644);
        builder.append_data(&mut header, format!("f{index}"), std::io::empty())?;
    }
    assert!(ArchiveSummary::validate(&builder.into_inner()?).is_err());
    Ok(())
}
