use std::{
    fs::{DirBuilder, File, Metadata, OpenOptions},
    io,
    path::{Path, PathBuf},
};

pub trait CanonicalizePath {
    type Error;

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, Self::Error>;
}

/// Opens a file exactly as [`OpenOptions::open`] does.
pub trait OpenFile {
    type Error;

    fn open(&self, options: &OpenOptions, path: &Path) -> Result<File, Self::Error>;
}

/// Creates a directory exactly as [`DirBuilder::create`] does.
pub trait CreateDirectory {
    type Error;

    fn create_directory(&self, builder: &DirBuilder, path: &Path) -> Result<(), Self::Error>;
}

/// Reads metadata exactly as [`std::fs::metadata`] does.
pub trait ReadMetadata {
    type Error;

    fn metadata(&self, path: &Path) -> Result<Metadata, Self::Error>;
}

/// Removes a directory tree exactly as [`std::fs::remove_dir_all`] does.
pub trait RemoveDirectoryAll {
    type Error;

    fn remove_dir_all(&self, path: &Path) -> Result<(), Self::Error>;
}

/// Removes a file exactly as [`std::fs::remove_file`] does.
pub trait RemoveFile {
    type Error;

    fn remove_file(&self, path: &Path) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFileSystem;

impl CanonicalizePath for NativeFileSystem {
    type Error = io::Error;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, Self::Error> {
        path.canonicalize()
    }
}

impl OpenFile for NativeFileSystem {
    type Error = io::Error;
    fn open(&self, options: &OpenOptions, path: &Path) -> Result<File, Self::Error> {
        options.open(path)
    }
}

impl CreateDirectory for NativeFileSystem {
    type Error = io::Error;
    fn create_directory(&self, builder: &DirBuilder, path: &Path) -> Result<(), Self::Error> {
        builder.create(path)
    }
}

impl ReadMetadata for NativeFileSystem {
    type Error = io::Error;
    fn metadata(&self, path: &Path) -> Result<Metadata, Self::Error> {
        std::fs::metadata(path)
    }
}

impl RemoveDirectoryAll for NativeFileSystem {
    type Error = io::Error;
    fn remove_dir_all(&self, path: &Path) -> Result<(), Self::Error> {
        std::fs::remove_dir_all(path)
    }
}

impl RemoveFile for NativeFileSystem {
    type Error = io::Error;
    fn remove_file(&self, path: &Path) -> Result<(), Self::Error> {
        std::fs::remove_file(path)
    }
}

/// The file operations store-backed resume uses to build and remove its temporary configuration directory.
pub trait SessionFileSystem:
    CanonicalizePath<Error = io::Error>
    + OpenFile<Error = io::Error>
    + CreateDirectory<Error = io::Error>
    + ReadMetadata<Error = io::Error>
    + RemoveDirectoryAll<Error = io::Error>
    + RemoveFile<Error = io::Error>
    + Send
    + Sync
    + 'static
{
}

impl<T> SessionFileSystem for T where
    T: CanonicalizePath<Error = io::Error>
        + OpenFile<Error = io::Error>
        + CreateDirectory<Error = io::Error>
        + ReadMetadata<Error = io::Error>
        + RemoveDirectoryAll<Error = io::Error>
        + RemoveFile<Error = io::Error>
        + Send
        + Sync
        + 'static
{
}
