use std::os::fd::{AsRawFd, OwnedFd};

use super::{descriptor_limit, keep_only_stdio, mark_close_on_exec, passes_only_stdio};
use crate::ArgvStyle;

/// The write end of a new pipe, made inheritable as a descriptor another library opened without
/// `O_CLOEXEC` would be.
fn inheritable_descriptor() -> std::io::Result<OwnedFd> {
    let (_reader, writer) = std::io::pipe()?;
    let writer = OwnedFd::from(writer);
    // SAFETY: `F_SETFD` only changes the flags of a descriptor this function owns.
    let cleared = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFD, 0) };
    assert_eq!(cleared, 0);
    Ok(writer)
}

fn is_close_on_exec(descriptor: &OwnedFd) -> bool {
    // SAFETY: `F_GETFD` only reads the flags of a descriptor the caller owns.
    let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
    assert_ne!(flags, -1);
    flags & libc::FD_CLOEXEC != 0
}

#[test]
fn the_python_dialect_passes_only_stdio_everywhere_and_typescript_only_on_apple() {
    assert!(passes_only_stdio(ArgvStyle::Python));
    assert_eq!(passes_only_stdio(ArgvStyle::TypeScript), cfg!(target_vendor = "apple"));
}

#[test]
fn the_limit_reaches_past_stdio() {
    assert!(descriptor_limit() > 2);
}

#[test]
fn keeping_only_stdio_marks_an_inheritable_descriptor_close_on_exec() -> std::io::Result<()> {
    let descriptor = inheritable_descriptor()?;
    assert!(!is_close_on_exec(&descriptor));
    keep_only_stdio(descriptor_limit())?;
    assert!(is_close_on_exec(&descriptor));
    Ok(())
}

#[test]
fn marking_a_range_marks_each_open_descriptor_in_it() -> std::io::Result<()> {
    let descriptor = inheritable_descriptor()?;
    let raw = descriptor.as_raw_fd();
    mark_close_on_exec(raw, raw + 1);
    assert!(is_close_on_exec(&descriptor));
    Ok(())
}
