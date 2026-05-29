use std::collections::BTreeMap;

use super::errors::BundleLoadError;
use super::manifest::{max_archive_bytes, normalize_member_path, MANIFEST_DEPRECATED_FILENAME, MANIFEST_FILENAME};

const TAR_BLOCK: usize = 512;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundleArchive {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl BundleArchive {
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(&normalize_member_path(path)).map(Vec::as_slice)
    }

    pub fn insert(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.files.insert(normalize_member_path(&path.into()), bytes);
    }

    pub fn paths(&self) -> impl Iterator<Item = &String> {
        self.files.keys()
    }
}

pub fn extract_archive(archive_bytes: &[u8]) -> Result<BundleArchive, BundleLoadError> {
    if archive_bytes.len() > max_archive_bytes() {
        return Err(BundleLoadError::ArchiveTooLarge {
            size: archive_bytes.len(),
            limit: max_archive_bytes(),
        });
    }

    let tar_bytes = if archive_bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(BundleLoadError::Archive(
            "gzip-compressed bundles require flate2 (not a direct dependency of trogon-mcp-gateway); \
             publish uncompressed tar for loader tests or add flate2 to Cargo.toml"
                .into(),
        ));
    } else {
        archive_bytes
    };

    extract_tar(tar_bytes)
}

pub fn build_tar(archive: &BundleArchive) -> Vec<u8> {
    let mut out = Vec::new();
    for (path, content) in &archive.files {
        write_tar_entry(&mut out, path, content);
    }
    out.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));
    out
}

fn extract_tar(bytes: &[u8]) -> Result<BundleArchive, BundleLoadError> {
    if bytes.len() % TAR_BLOCK != 0 {
        return Err(BundleLoadError::Archive(format!(
            "tar archive size {} is not a multiple of {TAR_BLOCK}",
            bytes.len()
        )));
    }

    let mut archive = BundleArchive::default();
    let mut offset = 0usize;
    while offset + TAR_BLOCK <= bytes.len() {
        let header = &bytes[offset..offset + TAR_BLOCK];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let path = parse_tar_path(header)?;
        let size = parse_tar_size(header)?;
        offset += TAR_BLOCK;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| BundleLoadError::Archive("tar entry size overflow".into()))?;
        if end > bytes.len() {
            return Err(BundleLoadError::Archive(format!(
                "tar entry `{path}` extends past archive end"
            )));
        }
        let content = bytes[offset..end].to_vec();
        offset = end;
        if size % TAR_BLOCK != 0 {
            offset += TAR_BLOCK - (size % TAR_BLOCK);
        }
        archive.insert(path, content);
    }
    Ok(archive)
}

fn parse_tar_path(header: &[u8]) -> Result<String, BundleLoadError> {
    let name = read_tar_field(&header[0..100])?;
    let prefix = read_tar_field(&header[345..500]).unwrap_or_default();
    let path = if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    };
    if path.is_empty() {
        return Err(BundleLoadError::Archive("tar entry missing path".into()));
    }
    Ok(normalize_member_path(&path))
}

fn parse_tar_size(header: &[u8]) -> Result<usize, BundleLoadError> {
    let field = read_tar_field(&header[124..136])?;
    let size = u64::from_str_radix(field.trim(), 8)
        .map_err(|error| BundleLoadError::Archive(format!("tar size parse: {error}")))?;
    usize::try_from(size)
        .map_err(|_| BundleLoadError::Archive(format!("tar entry size {size} does not fit in usize")))
}

fn read_tar_field(bytes: &[u8]) -> Result<String, BundleLoadError> {
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..end]).trim().to_string())
}

fn write_tar_entry(out: &mut Vec<u8>, path: &str, content: &[u8]) {
    let mut header = [0u8; TAR_BLOCK];
    write_field(&mut header[0..100], path);
    write_octal(&mut header[100..108], 0o644);
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], content.len() as u64);
    write_octal(&mut header[136..148], 0);
    header[156] = b'0';
    write_field(&mut header[257..265], "ustar");
    write_field(&mut header[265..269], "00");
    let checksum = header.iter().map(|byte| *byte as u64).sum::<u64>();
    write_octal(&mut header[148..156], checksum);
    out.extend_from_slice(&header);
    out.extend_from_slice(content);
    let padding = (TAR_BLOCK - (content.len() % TAR_BLOCK)) % TAR_BLOCK;
    out.extend(std::iter::repeat_n(0u8, padding));
}

fn write_field(target: &mut [u8], value: &str) {
    let bytes = value.as_bytes();
    let len = bytes.len().min(target.len());
    target[..len].copy_from_slice(&bytes[..len]);
}

fn write_octal(target: &mut [u8], value: u64) {
    let formatted = format!("{value:o}");
    write_field(target, &formatted);
}

pub fn resolve_manifest(archive: &BundleArchive) -> Result<(&str, &[u8]), BundleLoadError> {
    if let Some(bytes) = archive.get(MANIFEST_FILENAME) {
        return Ok((MANIFEST_FILENAME, bytes));
    }
    if let Some(bytes) = archive.get(MANIFEST_DEPRECATED_FILENAME) {
        return Ok((MANIFEST_DEPRECATED_FILENAME, bytes));
    }
    Err(BundleLoadError::ManifestMissing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tar_round_trip_preserves_files() {
        let mut archive = BundleArchive::default();
        archive.insert("manifest.toml", b"name = \"acme/demo\"".to_vec());
        archive.insert(
            "policies/rule.cel",
            b"mcp.method == \"tools/call\"".to_vec(),
        );
        let tar = build_tar(&archive);
        let extracted = extract_tar(&tar).expect("extract");
        assert_eq!(
            extracted.get("manifest.toml"),
            Some(b"name = \"acme/demo\"".as_slice())
        );
        assert_eq!(
            extracted.get("policies/rule.cel"),
            Some(b"mcp.method == \"tools/call\"".as_slice())
        );
    }

    #[test]
    fn gzip_archive_returns_actionable_error() {
        let error = extract_archive(&[0x1f, 0x8b, 0x08]).expect_err("gzip");
        assert!(matches!(error, BundleLoadError::Archive(_)));
    }
}
