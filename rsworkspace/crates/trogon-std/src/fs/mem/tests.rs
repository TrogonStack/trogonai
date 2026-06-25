use std::io;
use std::io::Write;
use std::path::Path;

use super::super::ReadFile;
use super::*;

#[test]
fn test_memfs_write_and_read() {
    let fs = MemFs::new();
    let path = Path::new("/test/file.txt");

    fs.write(path, "hello world").unwrap();
    let content = fs.read_to_string(path).unwrap();

    assert_eq!(content, "hello world");
}

#[test]
fn test_memfs_exists() {
    let fs = MemFs::new();
    let path = Path::new("/test/file.txt");

    assert!(!fs.exists(path));
    fs.write(path, "content").unwrap();
    assert!(fs.exists(path));
}

#[test]
fn test_memfs_not_found() {
    let fs = MemFs::new();
    let result = fs.read_to_string(Path::new("/nonexistent"));

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
}

#[test]
fn test_memfs_multiple_files() {
    let fs = MemFs::new();

    fs.write(Path::new("/a.txt"), "aaa").unwrap();
    fs.write(Path::new("/b.txt"), "bbb").unwrap();
    fs.write(Path::new("/c.txt"), "ccc").unwrap();

    assert_eq!(fs.read_to_string(Path::new("/a.txt")).unwrap(), "aaa");
    assert_eq!(fs.read_to_string(Path::new("/b.txt")).unwrap(), "bbb");
    assert_eq!(fs.read_to_string(Path::new("/c.txt")).unwrap(), "ccc");
    assert_eq!(fs.len(), 3);
}

#[test]
fn test_memfs_overwrite() {
    let fs = MemFs::new();
    let path = Path::new("/test.txt");

    fs.write(path, "v1").unwrap();
    assert_eq!(fs.read_to_string(path).unwrap(), "v1");

    fs.write(path, "v2").unwrap();
    assert_eq!(fs.read_to_string(path).unwrap(), "v2");

    assert_eq!(fs.len(), 1);
}

#[test]
fn test_memfs_insert_helper() {
    let fs = MemFs::new();
    fs.insert("/config.json", r#"{"key": "value"}"#);

    let content = fs.read_to_string(Path::new("/config.json")).unwrap();
    assert!(content.contains("key"));
}

#[test]
fn test_memfs_empty() {
    let fs = MemFs::new();
    assert!(fs.is_empty());
    assert_eq!(fs.len(), 0);

    fs.insert("/file.txt", "data");
    assert!(!fs.is_empty());
    assert_eq!(fs.len(), 1);
}

#[test]
fn test_memfs_default() {
    let fs = MemFs::default();
    assert!(fs.is_empty());
}

#[test]
fn test_memfs_unicode_content() {
    let fs = MemFs::new();
    let path = Path::new("/unicode.txt");
    let content = "Hello 世界 🌍 café";

    fs.write(path, content).unwrap();
    assert_eq!(fs.read_to_string(path).unwrap(), content);
}

#[test]
fn test_memfs_empty_content() {
    let fs = MemFs::new();
    let path = Path::new("/empty.txt");

    fs.write(path, "").unwrap();
    assert!(fs.exists(path));
    assert_eq!(fs.read_to_string(path).unwrap(), "");
}

#[test]
fn test_memfs_nested_paths() {
    let fs = MemFs::new();

    fs.insert("/a/b/c/deep.txt", "deep");
    fs.insert("/a/b/shallow.txt", "shallow");

    assert_eq!(fs.read_to_string(Path::new("/a/b/c/deep.txt")).unwrap(), "deep");
    assert_eq!(fs.read_to_string(Path::new("/a/b/shallow.txt")).unwrap(), "shallow");
}

#[test]
fn test_memfs_open_append_persists_writes() {
    let fs = MemFs::new();
    let path = Path::new("/log.txt");

    let mut w = fs.open_append(path).unwrap();
    w.write_all(b"line1\n").unwrap();
    w.write_all(b"line2\n").unwrap();
    drop(w);

    assert_eq!(fs.read_to_string(path).unwrap(), "line1\nline2\n");
}

#[test]
fn test_memfs_open_append_to_existing_file() {
    let fs = MemFs::new();
    let path = Path::new("/log.txt");
    fs.insert(path, "existing\n");

    let mut w = fs.open_append(path).unwrap();
    w.write_all(b"appended\n").unwrap();
    drop(w);

    assert_eq!(fs.read_to_string(path).unwrap(), "existing\nappended\n");
}

#[test]
fn test_memfs_create_dir_all() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/a/b/c")).unwrap();

    assert!(fs.dir_exists(Path::new("/a/b/c")));
    assert!(fs.dir_exists(Path::new("/a/b")));
    assert!(fs.dir_exists(Path::new("/a")));
    assert!(fs.dir_exists(Path::new("/")));
}

#[test]
fn test_memfs_create_dir_all_fails_when_component_is_file() {
    let fs = MemFs::new();
    fs.insert("/a/b", "file content");

    let err = fs.create_dir_all(Path::new("/a/b/c")).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn test_memfs_create_dir_all_fails_when_target_is_file() {
    let fs = MemFs::new();
    fs.insert("/x", "data");

    let err = fs.create_dir_all(Path::new("/x")).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn test_memfs_create_dir_all_idempotent() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/x/y")).unwrap();
    fs.create_dir_all(Path::new("/x/y")).unwrap();

    assert!(fs.dir_exists(Path::new("/x/y")));
}

#[test]
fn test_generic_function_with_memfs() {
    fn read_config<F: ReadFile>(fs: &F, path: &Path) -> String {
        fs.read_to_string(path).unwrap_or_else(|_| "{}".to_string())
    }

    let fs = MemFs::new();
    fs.insert("/config.json", r#"{"port": 8080}"#);

    assert_eq!(read_config(&fs, Path::new("/config.json")), r#"{"port": 8080}"#);
    assert_eq!(read_config(&fs, Path::new("/missing.json")), "{}");
}
