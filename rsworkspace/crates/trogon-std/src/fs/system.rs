use std::path::Path;

use super::{ExistsFile, ReadFile, WriteFile};

/// Zero-sized type — delegates to `std::fs`.
pub struct SystemFs;

impl ReadFile for SystemFs {
    #[inline]
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
}

impl WriteFile for SystemFs {
    #[inline]
    fn write(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        std::fs::write(path, contents)
    }
}

impl ExistsFile for SystemFs {
    #[inline]
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn test_system_fs_nonexistent_file() {
        let fs = SystemFs;
        assert!(!fs.exists(Path::new("/nonexistent_trogonstd_test_file_12345")));
    }

    #[test]
    fn test_generic_function_with_system_fs() {
        fn read_config<F: ReadFile>(fs: &F, path: &Path) -> String {
            fs.read_to_string(path).unwrap_or_else(|_| "{}".to_string())
        }

        let fs = SystemFs;
        assert_eq!(read_config(&fs, Path::new("/nonexistent_12345")), "{}");
    }
}
