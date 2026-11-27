//! File verbs of the fake worker, served from the host filesystem.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{
    Base64Data, FileKind, FileOp, FileOpResponse, FilePresence, FileRecursion, FileResultTruncation, FileStat,
    SearchMatch,
};

#[derive(Serialize, Deserialize)]
struct Packed {
    path: String,
    data: String,
}

pub fn write(path: &str, content: &[u8]) -> io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}

fn b64(bytes: &[u8]) -> Result<Base64Data, Box<dyn std::error::Error>> {
    Ok(Base64Data::new(STANDARD.encode(bytes))?)
}

fn stat_of(path: &Path, name: &str, shown: &str) -> io::Result<FileStat> {
    let metadata = fs::metadata(path)?;
    Ok(FileStat {
        path: shown.to_owned(),
        name: name.to_owned(),
        kind: if metadata.is_dir() {
            FileKind::Directory
        } else {
            FileKind::File
        },
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        mode: None,
        modified_unix_ms: None,
        symlink_target: None,
    })
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn not_found(path: &str) -> FileOpResponse {
    FileOpResponse::NotFound { path: path.to_owned() }
}

pub fn serve(op: FileOp) -> Result<FileOpResponse, Box<dyn std::error::Error>> {
    Ok(match op {
        FileOp::Mkdir { path, .. } => {
            let existed = Path::new(&path).is_dir();
            fs::create_dir_all(&path)?;
            FileOpResponse::Created {
                already_existed: FilePresence::from(existed),
            }
        }
        FileOp::Write { path, data, .. } => {
            let bytes = STANDARD.decode(data.as_str())?;
            write(&path, &bytes)?;
            let size = bytes.len() as u64;
            FileOpResponse::Written { bytes: size, size }
        }
        FileOp::Read { path, .. } => match fs::read(&path) {
            Ok(bytes) => FileOpResponse::Content {
                data: b64(&bytes)?,
                offset: 0,
                total_size: bytes.len() as u64,
                eof: true.into(),
                truncated: FileResultTruncation::Complete,
            },
            Err(_) => not_found(&path),
        },
        FileOp::Stat { path } => {
            let name = Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            match stat_of(Path::new(&path), &name, &path) {
                Ok(stat) => FileOpResponse::Stat { stat },
                Err(_) => not_found(&path),
            }
        }
        FileOp::List { path, .. } => match fs::read_dir(&path) {
            Ok(entries) => {
                let mut stats = Vec::new();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    stats.push(stat_of(&entry.path(), &name, &name)?);
                }
                FileOpResponse::Listing {
                    entries: stats,
                    truncated: FileResultTruncation::Complete,
                }
            }
            Err(_) => not_found(&path),
        },
        FileOp::Rename { from, to, .. } => match fs::rename(&from, &to) {
            Ok(()) => FileOpResponse::Renamed,
            Err(_) => not_found(&from),
        },
        FileOp::Remove { path, recursive } => {
            let target = Path::new(&path);
            let existed = target.exists();
            match recursive {
                FileRecursion::Recursive if existed => fs::remove_dir_all(target)?,
                FileRecursion::Flat if existed => fs::remove_file(target)?,
                _ => {}
            }
            FileOpResponse::Removed {
                existed: FilePresence::from(existed),
            }
        }
        FileOp::Search {
            path, glob, pattern, ..
        } => {
            let mut matches = Vec::new();
            for file in walk(Path::new(&path)) {
                let shown = file.to_string_lossy().into_owned();
                let suffix = glob.as_deref().map(|glob| glob.trim_start_matches('*'));
                if suffix.is_some_and(|suffix| !shown.ends_with(suffix)) {
                    continue;
                }
                let Some(needle) = pattern.as_deref() else {
                    matches.push(SearchMatch {
                        path: shown,
                        line: None,
                        text: None,
                    });
                    continue;
                };
                let text = fs::read_to_string(&file).unwrap_or_default();
                for (number, line) in text.lines().enumerate() {
                    if line.contains(needle) {
                        matches.push(SearchMatch {
                            path: shown.clone(),
                            line: Some(number as u64 + 1),
                            text: Some(line.to_owned()),
                        });
                    }
                }
            }
            FileOpResponse::SearchResults {
                matches,
                truncated: FileResultTruncation::Complete,
                files_scanned: 0,
            }
        }
        FileOp::ArchiveGet { path, .. } => {
            let root = PathBuf::from(&path);
            let mut packed = Vec::new();
            let mut bytes = 0u64;
            for file in walk(&root) {
                let content = fs::read(&file)?;
                bytes += content.len() as u64;
                packed.push(Packed {
                    path: file.strip_prefix(&root)?.to_string_lossy().into_owned(),
                    data: STANDARD.encode(content),
                });
            }
            FileOpResponse::Archive {
                data: b64(&serde_json::to_vec(&packed)?)?,
                entries: packed.len() as u64,
                bytes,
            }
        }
        FileOp::ArchivePut { path, data } => {
            let packed: Vec<Packed> = serde_json::from_slice(&STANDARD.decode(data.as_str())?)?;
            let mut bytes = 0u64;
            for entry in &packed {
                let content = STANDARD.decode(&entry.data)?;
                bytes += content.len() as u64;
                write(&format!("{path}/{}", entry.path), &content)?;
            }
            FileOpResponse::Extracted {
                entries: packed.len() as u64,
                bytes,
            }
        }
        _ => FileOpResponse::Unsupported {
            message: "the fake worker serves the file verbs the suite uses".to_owned(),
        },
    })
}
