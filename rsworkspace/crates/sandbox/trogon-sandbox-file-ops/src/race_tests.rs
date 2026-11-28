//! A path component swapped for a symlink after the path check ran, as the helpers would see it.

use std::error::Error;
use std::fs;
use std::io;

use tempfile::TempDir;
use trogon_sandbox_contract::FileWriteTruncation;

use crate::containment::opened_inside;
use crate::fs_io::{open_read, write_at, write_whole};
use crate::{FsRoot, dirfd};

type TestResult = Result<(), Box<dyn Error>>;

/// A root whose `swapped` entry leads to a directory outside it holding `secret` and `victim`.
fn swapped_root() -> Result<(TempDir, FsRoot, TempDir), Box<dyn Error>> {
    let dir = TempDir::new()?;
    let fs_root = FsRoot::new(dir.path(), None)?;
    let outside = TempDir::new()?;
    fs::write(outside.path().join("secret"), b"keep")?;
    fs::create_dir(outside.path().join("victim"))?;
    std::os::unix::fs::symlink(outside.path(), dir.path().join("swapped"))?;
    Ok((dir, fs_root, outside))
}

#[test]
fn a_descriptor_that_lands_outside_the_root_is_refused_before_any_change() -> TestResult {
    let (dir, fs_root, outside) = swapped_root()?;
    let raced = fs_root.root.join("swapped/secret");

    let error = write_at(&raced, b"x", 0, FileWriteTruncation::Truncate, None, &fs_root.root)
        .err()
        .ok_or("a raced ranged write must be refused")?;
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(fs::read(outside.path().join("secret"))?, b"keep");

    let error = write_whole(&raced, b"x", None, &fs_root.root)
        .err()
        .ok_or("a raced whole write must be refused")?;
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(fs::read(outside.path().join("secret"))?, b"keep");
    assert_eq!(fs::read_dir(outside.path())?.count(), 2);

    assert!(!opened_inside(&open_read(&raced)?, &fs_root.root)?);
    let inside = dir.path().join("inside.txt");
    fs::write(&inside, b"ok")?;
    assert!(opened_inside(&open_read(&inside)?, &fs_root.root)?);
    Ok(())
}

#[test]
fn a_refused_create_leaves_no_empty_file_behind() -> TestResult {
    let (dir, fs_root, _outside) = swapped_root()?;
    // A root that is not the real one makes the check after the open fail, which is the point
    // where a create has already made the file.
    let wrong_root = TempDir::new()?;
    let wrong = wrong_root.path().canonicalize()?;
    let error = write_at(
        &fs_root.root.join("fresh.txt"),
        b"x",
        0,
        FileWriteTruncation::Truncate,
        None,
        &wrong,
    )
    .err()
    .ok_or("a create checked against the wrong root must be refused")?;
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(!dir.path().join("fresh.txt").exists());

    fs::write(dir.path().join("old.txt"), b"keep")?;
    let refused = write_at(
        &fs_root.root.join("old.txt"),
        b"x",
        0,
        FileWriteTruncation::Truncate,
        None,
        &wrong,
    );
    assert!(refused.is_err());
    assert_eq!(fs::read(dir.path().join("old.txt"))?, b"keep");
    Ok(())
}

#[test]
fn a_swapped_parent_cannot_redirect_remove() -> TestResult {
    let (_dir, fs_root, outside) = swapped_root()?;
    let file = dirfd::remove(&fs_root.root.join("swapped/secret"), &fs_root.root, false)
        .err()
        .ok_or("removing through a swapped parent must be refused")?;
    assert_eq!(file.kind(), io::ErrorKind::InvalidInput);
    let tree = dirfd::remove(&fs_root.root.join("swapped/victim"), &fs_root.root, true)
        .err()
        .ok_or("removing a tree through a swapped parent must be refused")?;
    assert_eq!(tree.kind(), io::ErrorKind::InvalidInput);
    assert!(outside.path().join("secret").exists());
    assert!(outside.path().join("victim").exists());
    Ok(())
}

#[test]
fn a_swapped_parent_cannot_redirect_mkdir() -> TestResult {
    let (_dir, fs_root, outside) = swapped_root()?;
    for parents in [false, true] {
        let host = fs_root.root.join("swapped/new/deeper");
        let error = dirfd::mkdir(&fs_root.root, &host, parents)
            .err()
            .ok_or("mkdir through a swapped parent must be refused")?;
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "parents: {parents}");
    }
    assert!(!outside.path().join("new").exists());
    Ok(())
}

#[test]
fn a_swapped_parent_cannot_redirect_rename() -> TestResult {
    let (dir, fs_root, outside) = swapped_root()?;
    fs::write(dir.path().join("mine"), b"mine")?;
    let (is_source, error) = dirfd::rename(
        &fs_root.root.join("swapped/secret"),
        &fs_root.root.join("stolen"),
        true,
        &fs_root.root,
    )
    .err()
    .ok_or("a rename out of a swapped parent must be refused")?;
    assert!(is_source);
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    let (is_source, error) = dirfd::rename(
        &fs_root.root.join("mine"),
        &fs_root.root.join("swapped/planted"),
        true,
        &fs_root.root,
    )
    .err()
    .ok_or("a rename into a swapped parent must be refused")?;
    assert!(!is_source);
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(outside.path().join("secret").exists());
    assert!(!outside.path().join("planted").exists());
    assert!(dir.path().join("mine").exists());
    assert!(!dir.path().join("stolen").exists());
    Ok(())
}
