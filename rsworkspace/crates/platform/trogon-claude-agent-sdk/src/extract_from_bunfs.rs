//! `extractFromBunfs` from the `./extract` entrypoint of `@anthropic-ai/claude-agent-sdk` 0.3.323
//! (`extractFromBunfs.js`, shipped unbundled).
//!
//! A program packed into a Bun single-file executable reads its embedded files from a virtual file system whose
//! paths contain `$bunfs` (`~BUN` on Windows). Child processes cannot see that file system, so the embedded CLI is
//! copied to a real, per-user, content-addressed location before it is spawned. Any other path is returned as it
//! is, and every failure is reported as a warning while the original path is returned.

use std::{
    ffi::OsStr,
    fs::{DirBuilder, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    adapters::{
        ChangeMode, CreateDirectory, EmitWarning, EnvironmentVariable, ExtractHost, FileIdentity, HostIdentity,
        NativeExtractHost, OpenFile, OperatingSystem, ProcessId, RandomBytes, ReadFile, RealUserId, RemoveFile,
        RenamePath, StatLink, TemporaryDirectory,
    },
    node_path_join::{PathStyle, join},
};

const CHUNK: usize = 1 << 20;
const FAILURE_PREFIX: &str = "[claude-agent-sdk] Failed to extract CLI from $bunfs: ";
const FAILURE_SUFFIX: &str = ". Child processes cannot access $bunfs paths \u{2014} the CLI will likely fail to start.";
const HASH_DIRECTORY_MESSAGE: &str = "The claude-agent-sdk-<hash> folder under the temp directory is a symlink or not a directory, possibly planted; delete it and restart";

/// libuv's name and text for each errno `extractFromBunfs` can meet, which is how Node words `err.message`.
#[cfg(unix)]
const ERRNO_TABLE: &[(i32, &str, &str)] = &[
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
const ERRNO_TABLE: &[(i32, &str, &str)] = &[
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

fn errno_row(error: &io::Error) -> Option<&'static (i32, &'static str, &'static str)> {
    let raw = error.raw_os_error()?;
    ERRNO_TABLE.iter().find(|row| row.0 == raw)
}

/// Node's `err.code` for an error from a file system call.
fn error_code(error: &io::Error) -> Option<&'static str> {
    errno_row(error).map(|row| row.1)
}

/// Node's `err.message` for a failed file system call: `<code>: <text>, <syscall> '<path>' -> '<dest>'`.
fn fs_message(error: &io::Error, syscall: &str, path: Option<&Path>, destination: Option<&Path>) -> String {
    let mut message = match errno_row(error) {
        Some(row) => format!("{}: {}, {syscall}", row.1, row.2),
        None => format!("{error}, {syscall}"),
    };
    if let Some(path) = path {
        message.push_str(&format!(" '{}'", path.to_string_lossy()));
    }
    if let Some(destination) = destination {
        message.push_str(&format!(" -> '{}'", destination.to_string_lossy()));
    }
    message
}

fn failure<'a>(syscall: &'a str, path: &'a Path) -> impl Fn(io::Error) -> String + 'a {
    move |error| fs_message(&error, syscall, Some(path), None)
}

fn hex(bytes: impl Iterator<Item = u8>) -> String {
    bytes.map(|byte| format!("{byte:02x}")).collect()
}

fn owner_only_directories() -> DirBuilder {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
}

/// `O_RDONLY | O_NOFOLLOW | O_NONBLOCK`: a symlink at the path is refused and a planted FIFO cannot hang the open.
fn refuse_links(options: &mut OpenOptions) {
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(options, libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(not(unix))]
    let _ = options;
}

fn owner_only_file(options: &mut OpenOptions) {
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(options, 0o700);
    #[cfg(not(unix))]
    let _ = options;
}

fn link_count(metadata: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        std::os::unix::fs::MetadataExt::nlink(metadata)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        1
    }
}

fn owner_id(metadata: &Metadata) -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::fs::MetadataExt::uid(metadata)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn permission_bits(metadata: &Metadata) -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::fs::MetadataExt::mode(metadata) & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0o700
    }
}

/// Copy of `hasNoFileIdentity`: a stat file id that names no object.
fn has_no_file_id(id: i128) -> bool {
    id == 0 || id == -1 || id == i128::from(u64::MAX) || id == 1_i128 << 64
}

/// `isExtractedCopy`: true when `file` is a regular file, with no second hard link, whose bytes equal `content`.
fn is_extracted_copy<H: ExtractHost>(host: &H, file: &Path, content: &[u8]) -> bool {
    matches_content(host, file, content).unwrap_or(false)
}

fn matches_content<H: ExtractHost>(host: &H, file: &Path, content: &[u8]) -> io::Result<bool> {
    let mut options = OpenOptions::new();
    options.read(true);
    let mut id_at_path = None;
    if host.real_user_id().is_some() {
        refuse_links(&mut options);
    } else {
        // No usable O_NOFOLLOW: look at the path first and compare file ids after the open.
        let at_path = host.stat_link(file)?;
        if !at_path.is_file() {
            return Ok(false);
        }
        id_at_path = Some(host.file_id(&at_path));
    }
    let mut opened = host.open(&options, file)?;
    let metadata = opened.metadata()?;
    if !metadata.is_file() || link_count(&metadata) != 1 || metadata.len() != content.len() as u64 {
        return Ok(false);
    }
    if let Some(before) = id_at_path {
        let after = host.file_id(&metadata);
        if !has_no_file_id(before) && !has_no_file_id(after) && before != after {
            return Ok(false);
        }
    }
    let mut chunk = vec![0_u8; CHUNK];
    let mut offset = 0;
    while offset < content.len() {
        let read = opened.read(&mut chunk)?;
        if read == 0 || content.get(offset..offset + read) != Some(&chunk[..read]) {
            return Ok(false);
        }
        offset += read;
    }
    Ok(true)
}

/// `tmpdir()`: `CLAUDE_CODE_TMPDIR`, else `/tmp` on macOS, else the operating system's temporary directory.
fn temp_root<H: ExtractHost>(host: &H) -> String {
    if let Some(directory) = host
        .var_os(OsStr::new("CLAUDE_CODE_TMPDIR"))
        .filter(|value| !value.is_empty())
    {
        return directory.to_string_lossy().into_owned();
    }
    if host.operating_system() == OperatingSystem::Darwin {
        return "/tmp".to_owned();
    }
    host.temp_dir().to_string_lossy().into_owned()
}

/// `safeTempBase`: `{tmpdir}/claude-agent-sdk-{uid}`, or `{tmpdir}/claude-agent-sdk` on Windows. Where the platform
/// has user ids the directory is created owner-only and refused unless it is a directory this user owns.
fn safe_temp_base<H: ExtractHost>(host: &H, style: PathStyle) -> Result<String, String> {
    let uid = host.real_user_id();
    let leaf = if host.operating_system() == OperatingSystem::Win32 {
        "claude-agent-sdk".to_owned()
    } else {
        format!("claude-agent-sdk-{}", uid.unwrap_or(0))
    };
    let directory = join(style, &[&temp_root(host), &leaf]);
    let path = Path::new(&directory);
    let Some(uid) = uid else {
        // Windows: best-effort create; %TEMP% is already per-user.
        let _ = host.create_directory(&owner_only_directories(), path);
        return Ok(directory);
    };
    host.create_directory(&owner_only_directories(), path)
        .map_err(failure("mkdir", path))?;
    let metadata = host.stat_link(path).map_err(failure("lstat", path))?;
    if !metadata.is_dir() {
        return Err(format!(
            "Temp directory {directory} is not a directory (may be an attacker-planted symlink). Refusing to use it."
        ));
    }
    let owner = owner_id(&metadata);
    if owner != uid {
        return Err(format!(
            "Temp directory {directory} is owned by uid {owner}, expected {uid}. Refusing to use it \u{2014} another user may have pre-created it."
        ));
    }
    if permission_bits(&metadata) != 0o700 {
        host.change_mode(path, 0o700).map_err(failure("chmod", path))?;
    }
    Ok(directory)
}

/// What the rename left behind: the path, and whether this call's own file is now at it.
type Placed = Result<(PathBuf, bool), String>;

/// The rename failed. Windows refuses to rename onto an existing file (`EEXIST`, or `EPERM`/`EBUSY` while another
/// process has that executable mapped), so an identical copy already there is as good as ours and anything else is
/// removed and the rename retried once.
fn recover<H: ExtractHost>(host: &H, error: &io::Error, temporary: &Path, target: &Path, content: &[u8]) -> Placed {
    let original = fs_message(error, "rename", Some(temporary), Some(target));
    if !matches!(error_code(error), Some("EEXIST" | "EPERM" | "EBUSY")) {
        return Err(original);
    }
    if is_extracted_copy(host, target, content) {
        return Ok((target.to_path_buf(), false));
    }
    match host.remove_file(target) {
        Err(unlink) if error_code(&unlink) != Some("ENOENT") => {
            return Err(fs_message(&unlink, "unlink", Some(target), None));
        }
        _ => {}
    }
    match host.rename_path(temporary, target) {
        Ok(()) => Ok((target.to_path_buf(), true)),
        // A concurrent start may have put its own copy in place first.
        Err(_) if is_extracted_copy(host, target, content) => Ok((target.to_path_buf(), false)),
        Err(_) => Err(original),
    }
}

fn place<H: ExtractHost>(host: &H, mut file: File, content: &[u8], temporary: &Path, target: &Path) -> Placed {
    file.write_all(content)
        .map_err(|error| fs_message(&error, "write", None, None))?;
    host.change_file_mode(&file, 0o755)
        .map_err(|error| fs_message(&error, "fchmod", None, None))?;
    drop(file);
    match host.rename_path(temporary, target) {
        Ok(()) => Ok((target.to_path_buf(), true)),
        Err(error) => recover(host, &error, temporary, target, content),
    }
}

fn extract<H: ExtractHost>(host: &H, embedded: &Path) -> Result<PathBuf, String> {
    let content = host.read_file(embedded).map_err(|error| {
        if error_code(&error) == Some("EISDIR") {
            fs_message(&error, "read", None, None)
        } else {
            fs_message(&error, "open", Some(embedded), None)
        }
    })?;
    let digest = Sha256::digest(&content);
    let hash = hex(digest.iter().copied().take(8));
    let style = PathStyle::host();
    let base = safe_temp_base(host, style)?;
    let directory = PathBuf::from(join(style, &[&base, &format!("claude-agent-sdk-{hash}")]));
    // The embedded file's basename is kept: the spawn decides between running the file directly and running it
    // through a script runtime by extension, so a native binary must not come out named cli.js.
    let file_name = embedded
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let target = PathBuf::from(join(style, &[&directory.to_string_lossy(), &file_name]));
    host.create_directory(&owner_only_directories(), &directory)
        .map_err(failure("mkdir", &directory))?;
    // Recursive mkdir accepts an existing symlink to a directory; refuse it, or every path below would resolve
    // wherever the link points.
    if !host
        .stat_link(&directory)
        .map_err(failure("lstat", &directory))?
        .is_dir()
    {
        return Err(HASH_DIRECTORY_MESSAGE.to_owned());
    }
    if is_extracted_copy(host, &target, &content) {
        return Ok(target);
    }
    // An unguessable name opened exclusively: a name planted in advance is never written through or renamed into
    // place.
    let temporary = PathBuf::from(join(
        style,
        &[
            &directory.to_string_lossy(),
            &format!(
                "{file_name}.tmp.{}.{}",
                host.process_id(),
                hex(host.random_bytes().into_iter())
            ),
        ],
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    owner_only_file(&mut options);
    let file = host.open(&options, &temporary).map_err(failure("open", &temporary))?;
    let placed = place(host, file, &content, &temporary, &target);
    if !matches!(placed, Ok((_, true))) {
        // Best-effort cleanup of the unused temporary file.
        let _ = host.remove_file(&temporary);
    }
    placed.map(|(path, _)| path)
}

/// `extractFromBunfs(embeddedPath)` on a host. A path with neither `$bunfs` nor `~BUN` in it is returned as is;
/// otherwise the file is copied to `{tmpdir}/claude-agent-sdk-{uid}/claude-agent-sdk-{sha256[..16]}/<name>` (see
/// `safeTempBase` for the Windows root) and that path is returned. A copy that is already there is reused only
/// after it is read back and found to be a plain file with the same bytes. Any failure is reported through
/// [`EmitWarning`] with the upstream text and the input path is returned.
pub fn extract_from_bunfs_with<H: ExtractHost>(embedded_path: &Path, host: &H) -> PathBuf {
    let text = embedded_path.to_string_lossy();
    if !text.contains("$bunfs") && !text.contains("~BUN") {
        return embedded_path.to_path_buf();
    }
    match extract(host, embedded_path) {
        Ok(path) => path,
        Err(message) => {
            host.emit_warning(&format!("{FAILURE_PREFIX}{message}{FAILURE_SUFFIX}"));
            embedded_path.to_path_buf()
        }
    }
}

/// [`extract_from_bunfs_with`] on the native host: real file system, `CLAUDE_CODE_TMPDIR`, and the warning goes to
/// the `tracing` stream.
pub fn extract_from_bunfs(embedded_path: &Path) -> PathBuf {
    extract_from_bunfs_with(embedded_path, &NativeExtractHost)
}

#[cfg(all(test, unix))]
#[path = "extract_from_bunfs_tests.rs"]
mod tests;
