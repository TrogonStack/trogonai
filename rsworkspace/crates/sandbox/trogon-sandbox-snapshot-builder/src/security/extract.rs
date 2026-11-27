use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use ext4_view::{Ext4, FileType};
use flate2::bufread::GzDecoder;

use super::ExtractError;
use super::extract_files::{check_deadline, create_file, set_mode};
use super::extract_job::ExtractJob;
use super::limited_reader::LimitedReader;
use crate::constants::{COPY_CHUNK_BYTES, PATH_MAX_BYTES};

fn safe_name(name: &str) -> Result<(), ExtractError> {
    let single = matches!(
        Path::new(name).components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    );
    if name.is_empty() || name.contains(['\\', '\0']) || !single {
        return Err(ExtractError::Rejected("unsafe filesystem entry name"));
    }
    Ok(())
}

fn validate_link(parent: &Path, target: &str) -> Result<(), ExtractError> {
    let mut depth = if target.starts_with('/') {
        0
    } else {
        parent.components().count()
    };
    if target.len() > PATH_MAX_BYTES || target.contains(['\\', '\0']) {
        return Err(ExtractError::Rejected("unsafe symbolic link target"));
    }
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                depth = depth
                    .checked_sub(1)
                    .ok_or(ExtractError::Rejected("symbolic link escapes the guest root"))?;
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

/// Inflates the rootfs when it is compressed and returns the path of the plain ext4 image.
fn inflate(source: &Path, job: &ExtractJob, expires: Instant) -> Result<PathBuf, ExtractError> {
    if !job.gzip {
        return Ok(source.to_owned());
    }
    let image = source.with_file_name("rootfs.ext4");
    let mut decoder = GzDecoder::new(BufReader::new(File::open(source)?));
    let mut output = create_file(&image)?;
    let mut buffer = [0; COPY_CHUNK_BYTES];
    let mut written = 0u64;
    loop {
        check_deadline(expires)?;
        let count = decoder.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        written = written
            .checked_add(count as u64)
            .ok_or(ExtractError::Rejected("rootfs byte budget overflow"))?;
        if written > job.limits.max_image_bytes {
            return Err(ExtractError::Rejected("rootfs image byte budget exceeded"));
        }
        output.write_all(&buffer[..count])?;
    }
    let mut remainder = decoder.into_inner();
    if remainder.read(&mut [0; 1])? != 0 {
        return Err(ExtractError::Rejected("trailing data in compressed rootfs"));
    }
    drop(output);
    Ok(image)
}

fn copy_file(
    filesystem: &Ext4,
    guest_path: &str,
    host_path: &Path,
    size: u64,
    expires: Instant,
) -> Result<(), ExtractError> {
    let mut input = filesystem.open(guest_path)?;
    let mut output = create_file(host_path)?;
    let mut buffer = [0; COPY_CHUNK_BYTES];
    let mut copied = 0u64;
    loop {
        check_deadline(expires)?;
        let count = input.read_bytes(&mut buffer)?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .ok_or(ExtractError::Rejected("file size overflow"))?;
        if copied > size {
            return Err(ExtractError::Rejected("filesystem file length changed"));
        }
        output.write_all(&buffer[..count])?;
    }
    if copied != size {
        return Err(ExtractError::Rejected("filesystem file length is inconsistent"));
    }
    drop(output);
    set_mode(host_path, 0o400)
}

pub(super) fn extract(source: &Path, destination: &Path, job: &ExtractJob) -> Result<(), ExtractError> {
    let expires = Instant::now() + Duration::from_millis(job.limits.timeout_ms);
    if source.parent() != destination.parent() {
        return Err(ExtractError::Rejected(
            "extraction paths must share a private work directory",
        ));
    }
    if std::fs::metadata(source)?.len() > job.limits.max_compressed_bytes {
        return Err(ExtractError::Rejected("compressed rootfs byte budget exceeded"));
    }
    let image = inflate(source, job, expires)?;
    let length = std::fs::metadata(&image)?.len();
    if length == 0 || length > job.limits.max_image_bytes {
        return Err(ExtractError::Rejected("rootfs image byte budget exceeded"));
    }
    let filesystem = Ext4::load(Box::new(LimitedReader {
        file: File::open(image)?,
        length,
        consumed: 0,
        budget: job.limits.max_image_bytes * 4 + job.limits.max_extracted_bytes * 2,
        expires,
    }))?;
    std::fs::create_dir(destination)?;
    set_mode(destination, 0o700)?;
    let mut directories = vec![destination.to_owned()];
    let mut pending = vec![(String::from("/"), PathBuf::new(), 0usize)];
    let mut entries = 0usize;
    let mut extracted = 0u64;
    while let Some((guest, relative, depth)) = pending.pop() {
        check_deadline(expires)?;
        for entry in filesystem.read_dir(guest.as_str())? {
            check_deadline(expires)?;
            entries = entries
                .checked_add(1)
                .ok_or(ExtractError::Rejected("entry budget overflow"))?;
            if entries > job.limits.max_entries {
                return Err(ExtractError::Rejected("filesystem entry budget exceeded"));
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name.as_str()?;
            if matches!(name, "." | "..") {
                continue;
            }
            safe_name(name)?;
            let guest_path = if guest == "/" {
                format!("/{name}")
            } else {
                format!("{guest}/{name}")
            };
            if depth >= job.limits.max_depth || guest_path.len() > PATH_MAX_BYTES {
                return Err(ExtractError::Rejected("filesystem path depth budget exceeded"));
            }
            let relative_path = relative.join(name);
            let host_path = destination.join(&relative_path);
            let metadata = entry.metadata()?;
            let kind = metadata.file_type();
            if kind != entry.file_type()? {
                return Err(ExtractError::Rejected("filesystem entry type is inconsistent"));
            }
            match kind {
                FileType::Directory => {
                    std::fs::create_dir(&host_path)?;
                    set_mode(&host_path, 0o700)?;
                    directories.push(host_path);
                    pending.push((guest_path, relative_path, depth + 1));
                }
                FileType::Regular => {
                    let size = metadata.len();
                    if size > job.limits.max_file_bytes {
                        return Err(ExtractError::Rejected("filesystem file byte budget exceeded"));
                    }
                    extracted = extracted
                        .checked_add(size)
                        .ok_or(ExtractError::Rejected("extracted byte budget overflow"))?;
                    if extracted > job.limits.max_extracted_bytes {
                        return Err(ExtractError::Rejected("extracted filesystem byte budget exceeded"));
                    }
                    copy_file(&filesystem, &guest_path, &host_path, size, expires)?;
                }
                FileType::Symlink => {
                    let target = filesystem.read_link(guest_path.as_str())?;
                    validate_link(&relative, target.to_str()?)?;
                }
                _ => {
                    return Err(ExtractError::Rejected(
                        "special filesystem nodes cannot be scanned safely",
                    ));
                }
            }
        }
    }
    for path in directories.iter().rev() {
        check_deadline(expires)?;
        set_mode(path, 0o500)?;
    }
    Ok(())
}
