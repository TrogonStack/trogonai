use crate::dir_look::DirLook;
use crate::path_on_disk::PathOnDisk;
use crate::profile_disk::ProfileDisk;

/// The disk of the machine the check runs on.
pub struct HostDisk;

fn absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

impl ProfileDisk for HostDisk {
    fn resolve(&self, native_path: &str) -> PathOnDisk {
        match std::fs::canonicalize(native_path) {
            Ok(real) => PathOnDisk::Resolved(real.to_string_lossy().into_owned()),
            Err(error) if absent(&error) => PathOnDisk::Absent,
            Err(error) => PathOnDisk::Failed(error.kind().to_string()),
        }
    }

    fn look(&self, native_dir: &str) -> DirLook {
        let path = std::path::Path::new(native_dir);
        let meta = match path.symlink_metadata() {
            Ok(meta) => meta,
            Err(error) if absent(&error) => return DirLook::Absent,
            Err(error) => return DirLook::Failed(error.kind().to_string()),
        };
        if meta.file_type().is_symlink() {
            return DirLook::Link;
        }
        if !meta.is_dir() {
            return DirLook::NotDirectory;
        }
        match std::fs::read_dir(path) {
            Ok(entries) => DirLook::Names(
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect(),
            ),
            Err(error) if absent(&error) => DirLook::Absent,
            Err(error) => DirLook::Failed(error.kind().to_string()),
        }
    }
}
