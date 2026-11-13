use std::{
    ffi::{OsStr, OsString},
    fs::{DirBuilder, File, Metadata, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use super::{
    CreateDirectory, EnvironmentVariable, HostIdentity, NativeFileSystem, NativeHost, OpenFile, OperatingSystem,
    ReadFile, RealUserId, RemoveFile, SystemEnvironment, TemporaryDirectory,
};

/// Reads metadata without following a final symbolic link, exactly as [`std::fs::symlink_metadata`] does.
pub trait StatLink {
    fn stat_link(&self, path: &Path) -> io::Result<Metadata>;
}

/// Renames a file exactly as [`std::fs::rename`] does.
pub trait RenamePath {
    fn rename_path(&self, from: &Path, to: &Path) -> io::Result<()>;
}

/// Sets permission bits as `chmod` and `fchmod` do; a no-op where the platform has none.
pub trait ChangeMode {
    fn change_mode(&self, path: &Path, mode: u32) -> io::Result<()>;

    /// Through the open file, so a name swapped for a symbolic link cannot redirect it.
    fn change_file_mode(&self, file: &File, mode: u32) -> io::Result<()>;
}

/// The id of the running process, as `process.pid` reports it.
pub trait ProcessId {
    fn process_id(&self) -> u32;
}

/// Eight unguessable bytes, as `crypto.randomBytes(8)` returns them.
pub trait RandomBytes {
    fn random_bytes(&self) -> [u8; 8];
}

/// The file id a `stat` call reports for a metadata value, as Node's `st.ino` does with `bigint: true`. A value that
/// names no object (`0`, `-1`, all ones, `1 << 64`) is how the platform says it has no file id to give.
pub trait FileIdentity {
    fn file_id(&self, metadata: &Metadata) -> i128;
}

/// Prints one warning line, where Node's `console.warn` would.
pub trait EmitWarning {
    fn emit_warning(&self, message: &str);
}

/// Everything `extract_from_bunfs_with` asks of the host.
pub trait ExtractHost:
    ReadFile<Error = io::Error>
    + CreateDirectory<Error = io::Error>
    + OpenFile<Error = io::Error>
    + RemoveFile<Error = io::Error>
    + StatLink
    + RenamePath
    + ChangeMode
    + ProcessId
    + RandomBytes
    + FileIdentity
    + EmitWarning
    + EnvironmentVariable
    + TemporaryDirectory
    + HostIdentity
    + RealUserId
{
}

impl<T> ExtractHost for T where
    T: ReadFile<Error = io::Error>
        + CreateDirectory<Error = io::Error>
        + OpenFile<Error = io::Error>
        + RemoveFile<Error = io::Error>
        + StatLink
        + RenamePath
        + ChangeMode
        + ProcessId
        + RandomBytes
        + FileIdentity
        + EmitWarning
        + EnvironmentVariable
        + TemporaryDirectory
        + HostIdentity
        + RealUserId
{
}

/// The native host: the real file system, process, environment and warning stream.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeExtractHost;

impl ReadFile for NativeExtractHost {
    type Error = io::Error;

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, Self::Error> {
        NativeHost.read_file(path)
    }
}

impl CreateDirectory for NativeExtractHost {
    type Error = io::Error;

    fn create_directory(&self, builder: &DirBuilder, path: &Path) -> Result<(), Self::Error> {
        NativeFileSystem.create_directory(builder, path)
    }
}

impl OpenFile for NativeExtractHost {
    type Error = io::Error;

    fn open(&self, options: &OpenOptions, path: &Path) -> Result<File, Self::Error> {
        NativeFileSystem.open(options, path)
    }
}

impl RemoveFile for NativeExtractHost {
    type Error = io::Error;

    fn remove_file(&self, path: &Path) -> Result<(), Self::Error> {
        NativeFileSystem.remove_file(path)
    }
}

impl StatLink for NativeExtractHost {
    fn stat_link(&self, path: &Path) -> io::Result<Metadata> {
        std::fs::symlink_metadata(path)
    }
}

impl RenamePath for NativeExtractHost {
    fn rename_path(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }
}

impl ChangeMode for NativeExtractHost {
    #[cfg(unix)]
    fn change_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }

    #[cfg(unix)]
    fn change_file_mode(&self, file: &File, mode: u32) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        file.set_permissions(std::fs::Permissions::from_mode(mode))
    }

    #[cfg(not(unix))]
    fn change_mode(&self, _path: &Path, _mode: u32) -> io::Result<()> {
        Ok(())
    }

    #[cfg(not(unix))]
    fn change_file_mode(&self, _file: &File, _mode: u32) -> io::Result<()> {
        Ok(())
    }
}

impl ProcessId for NativeExtractHost {
    fn process_id(&self) -> u32 {
        std::process::id()
    }
}

impl RandomBytes for NativeExtractHost {
    /// A version 4 id carries fixed bits in bytes 6 and 8; the other eight bytes are all random.
    fn random_bytes(&self) -> [u8; 8] {
        let id = uuid::Uuid::new_v4();
        let bytes = id.as_bytes();
        [
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[7], bytes[9],
        ]
    }
}

impl FileIdentity for NativeExtractHost {
    #[cfg(unix)]
    fn file_id(&self, metadata: &Metadata) -> i128 {
        use std::os::unix::fs::MetadataExt;

        i128::from(metadata.ino())
    }

    #[cfg(not(unix))]
    fn file_id(&self, _metadata: &Metadata) -> i128 {
        0
    }
}

impl EmitWarning for NativeExtractHost {
    fn emit_warning(&self, message: &str) {
        tracing::warn!("{message}");
    }
}

impl EnvironmentVariable for NativeExtractHost {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        SystemEnvironment.var_os(key)
    }
}

impl TemporaryDirectory for NativeExtractHost {
    fn temp_dir(&self) -> PathBuf {
        SystemEnvironment.temp_dir()
    }
}

impl HostIdentity for NativeExtractHost {
    fn operating_system(&self) -> OperatingSystem {
        NativeHost.operating_system()
    }

    fn username(&self) -> Option<String> {
        NativeHost.username()
    }
}

impl RealUserId for NativeExtractHost {
    fn real_user_id(&self) -> Option<u32> {
        NativeHost.real_user_id()
    }
}
