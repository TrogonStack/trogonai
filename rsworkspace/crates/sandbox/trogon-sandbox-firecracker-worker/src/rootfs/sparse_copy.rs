//! Per-lease copies of the base rootfs.
//!
//! The base image is mostly empty space, so each lease's copy reads only the image's data
//! extents and leaves everything else, including all-zero blocks inside them, as holes. The
//! guest reads holes back as zeros, and a lease then costs the host only the blocks the image
//! really uses plus whatever the guest writes.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use rustix::fs::{SeekFrom, seek};
use rustix::io::Errno;

static ZEROS: [u8; BLOCK] = [0; BLOCK];

pub async fn copy_sparse(from: &Path, to: &Path) -> io::Result<u64> {
    let (from, to) = (from.to_path_buf(), to.to_path_buf());
    let bytes = std::fs::metadata(&from)?.len();
    let parent = to
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let space = crate::space::reserve(parent, bytes).map_err(io::Error::other)?;
    tokio::task::spawn_blocking(move || {
        let _space = space;
        copy_sparse_blocking(&from, &to)
    })
    .await
    .map_err(io::Error::other)?
}

fn copy_sparse_blocking(from: &Path, to: &Path) -> io::Result<u64> {
    let source = File::open(from)?;
    let target = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(to)?;
    target.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let length = source.metadata()?.len();
    target.set_len(length)?;
    let mut buffer = vec![0u8; CHUNK];
    let mut offset = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while let Some((data, hole)) = next_extent(&source, offset, length)? {
        copy_extent(&source, &target, data, hole, &mut buffer, deadline)?;
        offset = hole;
    }
    Ok(length)
}

/// The next `[data, hole)` range at or after `offset`. A filesystem that cannot report holes
/// reports the whole file as data, which the zero-block check still thins out.
fn next_extent(source: &File, offset: u64, length: u64) -> io::Result<Option<(u64, u64)>> {
    if offset >= length {
        return Ok(None);
    }
    let data = match seek(source, SeekFrom::Data(offset)) {
        Ok(data) => data,
        Err(Errno::NXIO) => return Ok(None),
        Err(Errno::INVAL) => return Ok(Some((offset, length))),
        Err(e) => return Err(e.into()),
    };
    let hole = seek(source, SeekFrom::Hole(data))?;
    Ok(Some((data, hole.min(length))))
}

fn copy_extent(
    source: &File,
    target: &File,
    start: u64,
    end: u64,
    buffer: &mut [u8],
    deadline: std::time::Instant,
) -> io::Result<()> {
    let mut position = start;
    while position < end {
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "private sparse copy exceeded its deadline",
            ));
        }
        let chunk = &mut buffer[..CHUNK.min((end - position) as usize)];
        source.read_exact_at(chunk, position)?;
        write_skipping_zeros(target, chunk, position)?;
        position += chunk.len() as u64;
    }
    Ok(())
}

fn write_skipping_zeros(target: &File, chunk: &[u8], at: u64) -> io::Result<()> {
    let mut start = 0;
    while start < chunk.len() {
        let zero = is_zero(block_at(chunk, start));
        let mut end = start + block_at(chunk, start).len();
        while end < chunk.len() && is_zero(block_at(chunk, end)) == zero {
            end += block_at(chunk, end).len();
        }
        if !zero {
            target.write_all_at(&chunk[start..end], at + start as u64)?;
        }
        start = end;
    }
    Ok(())
}

fn block_at(chunk: &[u8], start: usize) -> &[u8] {
    &chunk[start..chunk.len().min(start + BLOCK)]
}

fn is_zero(block: &[u8]) -> bool {
    block == &ZEROS[..block.len()]
}

#[cfg(test)]
mod tests;

mod constants;
use constants::{BLOCK, CHUNK};
