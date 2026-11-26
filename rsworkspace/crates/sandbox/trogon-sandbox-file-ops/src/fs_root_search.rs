use std::fs;
use std::path::PathBuf;

use regex::RegexBuilder;
use trogon_sandbox_contract::{
    FileCaseSensitivity, FileOp, FileOpResponse, FileResultTruncation, FileSearchSyntax, SearchMatch,
};

use crate::fs_io::sorted_entries;
use crate::fs_root::Reply;
use crate::glob_pattern::REGEX_SIZE_LIMIT;
use crate::{FileOpFailure, FsRoot, GlobPattern};

/// Longest line kept in a content match; longer lines are cut so one minified file cannot flood a
/// reply.
const MAX_MATCH_LINE_CHARS: usize = 500;

/// Total bytes one content search reads across files.
const MAX_SEARCH_SCAN_BYTES: u64 = 256 * 1024 * 1024;

/// The parameters of one search, borrowed from the request.
pub(crate) struct SearchRequest<'a> {
    pub(crate) path: &'a str,
    pub(crate) glob: Option<&'a str>,
    pub(crate) pattern: Option<&'a str>,
    pub(crate) syntax: FileSearchSyntax,
    pub(crate) case: FileCaseSensitivity,
    pub(crate) max_results: Option<u32>,
    pub(crate) max_file_bytes: Option<u64>,
}

impl FsRoot {
    pub(crate) fn search(&self, request: &SearchRequest<'_>) -> Reply<FileOpResponse> {
        let path = request.path;
        let io = |e| FileOpFailure::from_io(e, path);
        let base = self.resolve(path, true)?;
        let limit = request
            .max_results
            .unwrap_or(FileOp::DEFAULT_MAX_ENTRIES)
            .clamp(1, FileOp::MAX_ENTRIES) as usize;
        // A glob with no `/` matches the file name at any depth, the way a bare `*.rs` is used.
        let bare_glob = request.glob.is_some_and(|glob| !glob.contains('/'));
        let glob = request
            .glob
            .map(|glob| GlobPattern::new(glob).map_err(|e| FileOpFailure::invalid_request(e.to_string())))
            .transpose()?;
        let matcher = request
            .pattern
            .map(|pattern| {
                let source = if request.syntax.is_regex() {
                    pattern.to_string()
                } else {
                    regex::escape(pattern)
                };
                RegexBuilder::new(&source)
                    .case_insensitive(request.case.ignores_case())
                    .size_limit(REGEX_SIZE_LIMIT)
                    .build()
                    .map_err(|e| FileOpFailure::invalid_request(format!("bad pattern: {e}")))
            })
            .transpose()?;
        let file_cap = request.max_file_bytes.unwrap_or(FileOp::DEFAULT_SEARCH_FILE_BYTES);
        let metadata = fs::metadata(&base).map_err(io)?;

        let mut files = Vec::new();
        if metadata.is_dir() {
            collect_files(&base, "", &mut files).map_err(io)?;
        } else {
            let name = base
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            files.push((name, base.clone(), metadata.len()));
        }

        let mut matches = Vec::new();
        let mut truncated = false;
        let mut files_scanned = 0u64;
        let mut scanned_bytes = 0u64;
        'files: for (relative, host, length) in files {
            if let Some(glob) = &glob {
                let subject = if bare_glob {
                    relative.rsplit('/').next().unwrap_or(&relative)
                } else {
                    relative.as_str()
                };
                if !glob.is_match(subject) {
                    continue;
                }
            }
            let Some(matcher) = &matcher else {
                if matches.len() >= limit {
                    truncated = true;
                    break;
                }
                matches.push(SearchMatch {
                    path: relative,
                    line: None,
                    text: None,
                });
                continue;
            };
            if length > file_cap {
                continue;
            }
            scanned_bytes = scanned_bytes.saturating_add(length);
            if scanned_bytes > MAX_SEARCH_SCAN_BYTES {
                truncated = true;
                break;
            }
            // A file that cannot be read has nothing to match, and still counts as scanned.
            let bytes = fs::read(&host).unwrap_or_default();
            files_scanned += 1;
            if bytes.iter().take(8192).any(|byte| *byte == 0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                if matcher.is_match(line) {
                    if matches.len() >= limit {
                        truncated = true;
                        break 'files;
                    }
                    matches.push(SearchMatch {
                        path: relative.clone(),
                        line: Some(index as u64 + 1),
                        text: Some(line.chars().take(MAX_MATCH_LINE_CHARS).collect()),
                    });
                }
            }
        }
        Ok(FileOpResponse::SearchResults {
            matches,
            truncated: FileResultTruncation::from(truncated),
            files_scanned,
        })
    }
}

/// Regular files under `dir` in name order, as `(relative path, host path)`. Symlinks are not
/// followed, so a search cannot read outside the tree through one.
fn collect_files(dir: &std::path::Path, relative: &str, out: &mut Vec<(String, PathBuf, u64)>) -> std::io::Result<()> {
    for entry in sorted_entries(dir)? {
        let metadata = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        if metadata.is_dir() {
            collect_files(&entry.path(), &path, out)?;
        } else if metadata.is_file() {
            out.push((path, entry.path(), metadata.len()));
        }
    }
    Ok(())
}
