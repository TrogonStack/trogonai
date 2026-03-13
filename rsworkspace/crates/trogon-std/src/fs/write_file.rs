use std::io;
use std::path::Path;

pub trait WriteFile {
    fn write(&self, path: &Path, contents: &str) -> io::Result<()>;
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::WriteFile;
    use crate::fs::{ExistsFile, MemFs, ReadFile};

    #[test]
    fn write_then_read_returns_same_content() {
        let fs = MemFs::new();
        let path = Path::new("/output.txt");
        fs.write(path, "hello world").unwrap();

        assert_eq!(fs.read_to_string(path).unwrap(), "hello world");
    }

    #[test]
    fn write_is_idempotent_for_same_path() {
        let fs = MemFs::new();
        let path = Path::new("/file.txt");
        fs.write(path, "v1").unwrap();
        fs.write(path, "v2").unwrap();

        assert_eq!(fs.read_to_string(path).unwrap(), "v2");
        assert_eq!(fs.len(), 1, "overwrite must not create a second entry");
    }

    #[test]
    fn write_empty_string_creates_file() {
        let fs = MemFs::new();
        let path = Path::new("/empty.txt");
        fs.write(path, "").unwrap();

        assert!(fs.exists(path));
        assert_eq!(fs.read_to_string(path).unwrap(), "");
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn save<F: WriteFile>(fs: &F, path: &Path, data: &str) {
            fs.write(path, data).unwrap();
        }

        let fs = MemFs::new();
        save(&fs, Path::new("/saved.txt"), "saved");

        assert_eq!(fs.read_to_string(Path::new("/saved.txt")).unwrap(), "saved");
    }
}
