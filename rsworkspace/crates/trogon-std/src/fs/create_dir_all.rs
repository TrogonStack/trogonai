use std::io;
use std::path::Path;

pub trait CreateDirAll {
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::CreateDirAll;
    use crate::fs::MemFs;

    #[test]
    fn creates_target_and_all_ancestors() {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/a/b/c")).unwrap();

        assert!(fs.dir_exists(Path::new("/a/b/c")));
        assert!(fs.dir_exists(Path::new("/a/b")));
        assert!(fs.dir_exists(Path::new("/a")));
    }

    #[test]
    fn is_idempotent() {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/x/y")).unwrap();
        fs.create_dir_all(Path::new("/x/y")).unwrap();

        assert!(fs.dir_exists(Path::new("/x/y")));
    }

    #[test]
    fn errors_when_path_component_is_a_file() {
        let fs = MemFs::new();
        fs.insert("/a/b", "file content");

        let err = fs.create_dir_all(Path::new("/a/b/c")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn ensure_dir<F: CreateDirAll>(fs: &F, path: &Path) {
            fs.create_dir_all(path).unwrap();
        }

        let fs = MemFs::new();
        ensure_dir(&fs, Path::new("/logs/app"));

        assert!(fs.dir_exists(Path::new("/logs/app")));
    }
}
