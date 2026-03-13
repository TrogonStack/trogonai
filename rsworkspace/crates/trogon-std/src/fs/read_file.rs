use std::io;
use std::path::Path;

pub trait ReadFile {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::ReadFile;
    use crate::fs::{MemFs, WriteFile};

    #[test]
    fn read_returns_written_content() {
        let fs = MemFs::new();
        let path = Path::new("/data/config.toml");
        fs.write(path, "key = \"value\"").unwrap();

        assert_eq!(fs.read_to_string(path).unwrap(), "key = \"value\"");
    }

    #[test]
    fn read_missing_file_returns_not_found() {
        let fs = MemFs::new();

        let err = fs.read_to_string(Path::new("/missing.txt")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn read_pre_inserted_file() {
        let fs = MemFs::new();
        fs.insert("/seed.txt", "seeded content");

        assert_eq!(
            fs.read_to_string(Path::new("/seed.txt")).unwrap(),
            "seeded content"
        );
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn load<F: ReadFile>(fs: &F, path: &Path) -> Option<String> {
            fs.read_to_string(path).ok()
        }

        let fs = MemFs::new();
        fs.insert("/present.txt", "hello");

        assert_eq!(load(&fs, Path::new("/present.txt")), Some("hello".into()));
        assert_eq!(load(&fs, Path::new("/absent.txt")), None);
    }
}
