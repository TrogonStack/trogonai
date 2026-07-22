use std::io;
use std::path::Path;

pub trait OpenAppendFile {
    type Writer: io::Write + Send + 'static;
    fn open_append(&self, path: &Path) -> io::Result<Self::Writer>;
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use super::OpenAppendFile;
    use crate::fs::{MemFs, ReadFile};

    #[test]
    fn written_bytes_are_readable_after_drop() {
        let fs = MemFs::new();
        let path = Path::new("/log.txt");

        let mut w = fs.open_append(path).unwrap();
        w.write_all(b"entry\n").unwrap();
        drop(w);

        assert_eq!(fs.read_to_string(path).unwrap(), "entry\n");
    }

    #[test]
    fn multiple_writes_in_one_writer_accumulate() {
        let fs = MemFs::new();
        let path = Path::new("/multi.txt");

        let mut w = fs.open_append(path).unwrap();
        w.write_all(b"line1\n").unwrap();
        w.write_all(b"line2\n").unwrap();
        drop(w);

        assert_eq!(fs.read_to_string(path).unwrap(), "line1\nline2\n");
    }

    #[test]
    fn appends_to_pre_existing_content() {
        let fs = MemFs::new();
        let path = Path::new("/existing.txt");
        fs.insert(path, "old\n");

        let mut w = fs.open_append(path).unwrap();
        w.write_all(b"new\n").unwrap();
        drop(w);

        assert_eq!(fs.read_to_string(path).unwrap(), "old\nnew\n");
    }

    #[test]
    fn was_opened_is_recorded() {
        let fs = MemFs::new();
        let path = Path::new("/tracked.txt");

        assert!(!fs.was_opened(path));
        let _w = fs.open_append(path).unwrap();
        assert!(fs.was_opened(path));
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn append_line<F: OpenAppendFile>(fs: &F, path: &Path, line: &str) {
            let mut w = fs.open_append(path).unwrap();
            w.write_all(line.as_bytes()).unwrap();
        }

        let fs = MemFs::new();
        append_line(&fs, Path::new("/g.txt"), "hello\n");

        assert_eq!(fs.read_to_string(Path::new("/g.txt")).unwrap(), "hello\n");
    }
}
