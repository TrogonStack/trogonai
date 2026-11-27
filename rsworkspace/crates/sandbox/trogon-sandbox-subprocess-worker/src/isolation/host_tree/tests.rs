use nix::errno::Errno::{EACCES, EIO, ENOTDIR, EPERM, EROFS};

use super::*;

fn source_of(path: &Path) -> Result<HostSource, Box<dyn std::error::Error>> {
    let node = LinuxHostTree.stat(path)?.ok_or("exists")?;
    Ok(HostSource {
        path: path.to_path_buf(),
        id: node.id,
        kind: node.kind,
    })
}

#[test]
fn a_mode_closed_directory_of_this_user_can_hold_a_mount_point() -> Result<(), Box<dyn std::error::Error>> {
    assert!(mount_point_is_makeable(None, false));
    assert!(mount_point_is_makeable(Some(EACCES), true));
    assert!(!mount_point_is_makeable(Some(EACCES), false));
    assert!(!mount_point_is_makeable(Some(EROFS), true));
    assert!(!mount_point_is_makeable(Some(EPERM), true));
    assert!(mount_point_is_makeable(Some(EIO), false));
    assert!(mount_point_is_makeable(Some(ENOTDIR), false));
    Ok(())
}

#[test]
fn a_directory_that_cannot_be_opened_or_is_reached_through_a_link_is_planned() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    let real = dir.path().join("real");
    std::fs::create_dir(&real)?;
    std::os::unix::fs::symlink(&real, dir.path().join("link"))?;
    assert!(LinuxHostTree.can_create_in(&dir.path().join("missing")));
    assert!(LinuxHostTree.can_create_in(&dir.path().join("link")));
    assert!(LinuxHostTree.can_create_in(&real));
    Ok(())
}

#[test]
fn read_file_is_bounded_and_refuses_links() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("f");
    std::fs::write(&file, "0123456789")?;
    std::os::unix::fs::symlink(&file, dir.path().join("l"))?;
    assert_eq!(LinuxHostTree.read_file(&file, 4)?, Some(b"0123".to_vec()));
    assert!(LinuxHostTree.read_file(&dir.path().join("l"), 4).is_err());
    assert_eq!(LinuxHostTree.read_file(dir.path(), 4)?, None);
    assert_eq!(LinuxHostTree.read_file(&dir.path().join("none"), 4)?, None);
    Ok(())
}

#[test]
fn open_source_returns_the_planned_object() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("f");
    std::fs::write(&file, "x")?;
    let source = source_of(&file)?;
    assert!(open_source(&source).is_ok());
    Ok(())
}

#[test]
fn open_source_refuses_an_object_swapped_after_planning() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("f");
    let other = dir.path().join("other");
    std::fs::write(&file, "x")?;
    std::fs::write(&other, "y")?;
    let source = source_of(&file)?;

    std::fs::rename(&other, &file)?;
    let err = open_source(&source).err().ok_or("the call should fail")?;
    assert_eq!(err.kind(), "source-changed", "{err}");
    Ok(())
}

#[test]
fn open_source_refuses_a_symlink_swapped_in_after_planning() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("f");
    let elsewhere = dir.path().join("elsewhere");
    std::fs::write(&file, "x")?;
    std::fs::write(&elsewhere, "z")?;
    let source = source_of(&file)?;

    std::fs::remove_file(&file)?;
    std::os::unix::fs::symlink(&elsewhere, &file)?;
    let err = open_source(&source).err().ok_or("the call should fail")?;
    assert_eq!(err.kind(), "open-source", "{err}");
    Ok(())
}

#[test]
fn open_source_refuses_a_symlinked_parent_swapped_in_after_planning() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let real = dir.path().join("real");
    std::fs::create_dir(&real)?;
    std::fs::write(real.join("f"), "x")?;
    let moved = dir.path().join("moved");
    let source = source_of(&real.join("f"))?;

    std::fs::rename(&real, &moved)?;
    std::os::unix::fs::symlink(&moved, &real)?;
    let err = open_source(&source).err().ok_or("the call should fail")?;
    assert_eq!(err.kind(), "open-source", "{err}");
    Ok(())
}

#[test]
fn open_source_refuses_a_path_that_vanished() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("f");
    std::fs::write(&file, "x")?;
    let source = source_of(&file)?;
    std::fs::remove_file(&file)?;
    assert_eq!(
        open_source(&source).err().ok_or("the call should fail")?.kind(),
        "open-source"
    );
    Ok(())
}
