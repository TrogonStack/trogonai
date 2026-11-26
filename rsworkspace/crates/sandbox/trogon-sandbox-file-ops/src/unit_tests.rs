use std::error::Error;
use std::fs::{self, File};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::Path;

use tempfile::TempDir;

use crate::fs_io::{opened_inside, replace_file, require_inside, sync_parent, write_whole};
use crate::fs_root_archive::Tally;
use crate::{FsRoot, dirfd};

type TestResult = Result<(), Box<dyn Error>>;

struct ClosedDescriptor;

impl AsRawFd for ClosedDescriptor {
    fn as_raw_fd(&self) -> RawFd {
        -1
    }
}

fn directory(path: &Path) -> Result<OwnedFd, Box<dyn Error>> {
    Ok(OwnedFd::from(File::open(path)?))
}

#[test]
fn a_tally_refuses_more_entries_than_the_archive_limit() -> TestResult {
    let mut tally = Tally {
        cap: u64::MAX,
        ..Tally::default()
    };
    for _ in 0..100_000 {
        tally.add_file(0, "many")?;
    }
    assert!(tally.add_file(0, "many").is_err());
    Ok(())
}

#[test]
fn a_tally_refuses_more_bytes_than_its_cap() {
    let mut tally = Tally {
        cap: 4,
        ..Tally::default()
    };
    assert!(tally.add_file(5, "big").is_err());
}

#[test]
fn a_descriptor_outside_the_root_is_refused() -> TestResult {
    let root = TempDir::new()?;
    let elsewhere = TempDir::new()?;
    let file = File::create(elsewhere.path().join("f"))?;
    assert!(!opened_inside(&file, &root.path().canonicalize()?)?);
    assert!(require_inside(&file, &root.path().canonicalize()?).is_err());
    let fs_root = FsRoot::new(root.path(), None)?;
    assert!(fs_root.ensure_opened_inside(&file, "f").is_err());
    Ok(())
}

#[test]
fn an_unusable_descriptor_cannot_be_placed() -> TestResult {
    let root = TempDir::new()?;
    assert!(opened_inside(&ClosedDescriptor, root.path()).is_err());
    Ok(())
}

#[test]
fn rename_if_absent_refuses_to_replace_and_otherwise_moves() -> TestResult {
    let root = TempDir::new()?;
    fs::write(root.path().join("from"), b"1")?;
    fs::write(root.path().join("taken"), b"2")?;
    let from_dir = directory(root.path())?;
    let to_dir = directory(root.path())?;
    let refused = dirfd::rename_if_absent(&from_dir, "from".as_ref(), &to_dir, "taken".as_ref());
    assert!(matches!(refused, Err((false, e)) if e.kind() == std::io::ErrorKind::AlreadyExists));
    dirfd::rename_if_absent(&from_dir, "from".as_ref(), &to_dir, "free".as_ref()).map_err(|(_, e)| e)?;
    assert_eq!(fs::read(root.path().join("free"))?, b"1");
    assert!(!root.path().join("from").exists());
    Ok(())
}

#[test]
fn create_new_refuses_a_file_that_already_exists() -> TestResult {
    let root = TempDir::new()?;
    fs::write(root.path().join("there"), b"1")?;
    let parent = directory(root.path())?;
    assert!(dirfd::create_new(&parent, "there".as_ref()).is_err());
    let created = dirfd::create_new(&parent, "fresh".as_ref())?;
    created.discard();
    assert!(!root.path().join("fresh").exists());
    Ok(())
}

#[test]
fn a_leftover_temp_file_fails_the_swap_and_is_cleaned_up() -> TestResult {
    let root = TempDir::new()?;
    let root_path = root.path().canonicalize()?;
    let target = root_path.join("doc");
    fs::write(&target, b"old")?;
    let temp = root_path.join(format!(".doc.edit-{}", std::process::id()));
    fs::write(&temp, b"squatter")?;
    let original = fs::metadata(&target)?;
    assert!(replace_file(&target, b"new", &original, &root_path).is_err());
    assert!(!temp.exists());
    assert_eq!(fs::read(&target)?, b"old");
    Ok(())
}

#[test]
fn a_whole_write_under_a_file_fails_to_look_the_target_up() -> TestResult {
    let root = TempDir::new()?;
    let root_path = root.path().canonicalize()?;
    fs::write(root_path.join("plain"), b"x")?;
    assert!(write_whole(&root_path.join("plain/child"), b"y", None, &root_path).is_err());
    Ok(())
}

#[test]
fn discarding_an_existing_file_keeps_it_and_a_missing_directory_is_not_synced() -> TestResult {
    let root = TempDir::new()?;
    let root_path = root.path().canonicalize()?;
    fs::write(root_path.join("kept"), b"x")?;
    dirfd::open_for_write(&root_path.join("kept"), &root_path)?.discard();
    assert!(root_path.join("kept").exists());
    sync_parent(&root_path.join("absent"));
    Ok(())
}
