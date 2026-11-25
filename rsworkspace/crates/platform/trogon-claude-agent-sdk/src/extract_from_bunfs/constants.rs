pub(super) const CHUNK: usize = 1 << 20;

pub(super) const FAILURE_PREFIX: &str = "[claude-agent-sdk] Failed to extract CLI from $bunfs: ";

pub(super) const FAILURE_SUFFIX: &str =
    ". Child processes cannot access $bunfs paths \u{2014} the CLI will likely fail to start.";

pub(super) const HASH_DIRECTORY_MESSAGE: &str = "The claude-agent-sdk-<hash> folder under the temp directory is a symlink or not a directory, possibly planted; delete it and restart";

/// libuv's name and text for each errno `extractFromBunfs` can meet, which is how Node words `err.message`.
#[cfg(unix)]
pub(super) const ERRNO_TABLE: &[(i32, &str, &str)] = &[
    (libc::EPERM, "EPERM", "operation not permitted"),
    (libc::ENOENT, "ENOENT", "no such file or directory"),
    (libc::EIO, "EIO", "i/o error"),
    (libc::ENOMEM, "ENOMEM", "not enough memory"),
    (libc::EACCES, "EACCES", "permission denied"),
    (libc::EBUSY, "EBUSY", "resource busy or locked"),
    (libc::EEXIST, "EEXIST", "file already exists"),
    (libc::EXDEV, "EXDEV", "cross-device link not permitted"),
    (libc::ENOTDIR, "ENOTDIR", "not a directory"),
    (libc::EISDIR, "EISDIR", "illegal operation on a directory"),
    (libc::EINVAL, "EINVAL", "invalid argument"),
    (libc::EMFILE, "EMFILE", "too many open files"),
    (libc::ENOSPC, "ENOSPC", "no space left on device"),
    (libc::EROFS, "EROFS", "read-only file system"),
    (libc::ENAMETOOLONG, "ENAMETOOLONG", "name too long"),
    (libc::ENOTEMPTY, "ENOTEMPTY", "directory not empty"),
    (libc::ELOOP, "ELOOP", "too many symbolic links encountered"),
];

/// The same table keyed by the Win32 error codes libuv translates.
#[cfg(not(unix))]
pub(super) const ERRNO_TABLE: &[(i32, &str, &str)] = &[
    (5, "EPERM", "operation not permitted"),
    (2, "ENOENT", "no such file or directory"),
    (3, "ENOENT", "no such file or directory"),
    (8, "ENOMEM", "not enough memory"),
    (32, "EBUSY", "resource busy or locked"),
    (33, "EBUSY", "resource busy or locked"),
    (80, "EEXIST", "file already exists"),
    (183, "EEXIST", "file already exists"),
    (17, "EXDEV", "cross-device link not permitted"),
    (267, "ENOTDIR", "not a directory"),
    (87, "EINVAL", "invalid argument"),
    (4, "EMFILE", "too many open files"),
    (112, "ENOSPC", "no space left on device"),
    (206, "ENAMETOOLONG", "name too long"),
    (145, "ENOTEMPTY", "directory not empty"),
];
