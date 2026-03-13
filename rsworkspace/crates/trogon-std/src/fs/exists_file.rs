use std::path::Path;

pub trait ExistsFile {
    fn exists(&self, path: &Path) -> bool;
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::ExistsFile;
    use crate::fs::{MemFs, WriteFile};

    #[test]
    fn returns_false_for_absent_path() {
        let fs = MemFs::new();
        assert!(!fs.exists(Path::new("/not/there.txt")));
    }

    #[test]
    fn returns_true_after_write() {
        let fs = MemFs::new();
        let path = Path::new("/file.txt");
        fs.write(path, "content").unwrap();

        assert!(fs.exists(path));
    }

    #[test]
    fn returns_true_for_inserted_file() {
        let fs = MemFs::new();
        fs.insert("/seed.json", "{}");

        assert!(fs.exists(Path::new("/seed.json")));
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn check<F: ExistsFile>(fs: &F, path: &Path) -> bool {
            fs.exists(path)
        }

        let fs = MemFs::new();
        fs.insert("/present.txt", "data");

        assert!(check(&fs, Path::new("/present.txt")));
        assert!(!check(&fs, Path::new("/absent.txt")));
    }
}
