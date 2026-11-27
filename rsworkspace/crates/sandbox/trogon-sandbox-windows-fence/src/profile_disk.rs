use crate::dir_look::DirLook;
use crate::path_on_disk::PathOnDisk;

/// The look at the disk the check takes.
pub trait ProfileDisk {
    fn resolve(&self, native_path: &str) -> PathOnDisk;
    fn look(&self, native_dir: &str) -> DirLook;
}
