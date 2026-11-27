use std::io::{self, Read, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::SnapshotTransportError;

fn archive(dir: &Path) -> io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    builder.append_dir_all(".", dir)?;
    builder.into_inner()
}

fn compress(raw: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw)?;
    encoder.finish()
}

/// Archives a snapshot layout directory into one gzip-compressed tar blob.
pub fn pack_layout_dir(dir: &Path) -> Result<Vec<u8>, SnapshotTransportError> {
    let raw = archive(dir).map_err(|source| SnapshotTransportError::Pack {
        path: dir.to_path_buf(),
        source,
    })?;
    compress(&raw).map_err(SnapshotTransportError::Compress)
}

/// Inverse of [`pack_layout_dir`]: decompresses `blob` and unpacks it into `dir`.
pub fn unpack_layout_dir(blob: &[u8], dir: &Path) -> Result<(), SnapshotTransportError> {
    let mut raw = Vec::new();
    GzDecoder::new(blob)
        .read_to_end(&mut raw)
        .map_err(SnapshotTransportError::Decompress)?;
    tar::Archive::new(raw.as_slice())
        .unpack(dir)
        .map_err(|source| SnapshotTransportError::Unpack {
            path: dir.to_path_buf(),
            source,
        })
}
