use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;

use crate::ToolContext;
use crate::types::{ContentBlock, ImageSource, ToolOutput};

/// Build a collision-resistant sibling temp path for atomic writes.
///
/// MED-12: a deterministic `<file>.tmp` lets two concurrent writes to the same
/// target pick the same temp file and clobber each other on rename. Mixing in
/// the pid, a monotonic nanosecond clock, and a process-local counter makes the
/// name unique per in-flight write so the `write → rename` pairs never collide.
pub(crate) fn unique_tmp_path(target: &std::path::Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let ext = target.extension().and_then(|e| e.to_str()).unwrap_or("");
    target.with_extension(format!("{ext}.{pid}.{nanos}.{seq}.tmp"))
}

/// When `TROGON_FS_UNRESTRICTED=1` (or `true`), the typed file/dir tools may operate
/// on any path the OS user can access, not just under the session cwd. Opt-in: the
/// default stays cwd-contained. (Bash is already unrestricted.)
pub fn fs_unrestricted() -> bool {
    matches!(
        std::env::var("TROGON_FS_UNRESTRICTED").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

/// Resolve a directory path from the session cwd (supports `~`, relative, and absolute).
pub fn resolve_directory_target(cwd: &str, raw: &str) -> Result<std::path::PathBuf, String> {
    use std::path::{Path, PathBuf};

    let raw = raw.trim();
    let target = if raw.is_empty() {
        std::env::var("HOME")
            .map(PathBuf::from)
            .map_err(|_| "HOME is not set".to_string())?
    } else if raw.starts_with('~') {
        let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
        PathBuf::from(home).join(raw.trim_start_matches('~').trim_start_matches('/'))
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        Path::new(cwd).join(raw)
    };

    let attempted = target.clone();
    let canonical = target
        .canonicalize()
        .map_err(|e| format!("cd: {} ({e})", attempted.display()))
        .and_then(|p| {
            if p.is_dir() {
                Ok(p)
            } else {
                Err(format!("not a directory: {}", attempted.display()))
            }
        })?;

    // Boundary check: the resolved (symlink-canonicalized) target must stay under the
    // canonicalized cwd — unless TROGON_FS_UNRESTRICTED lets the tools roam freely.
    if !fs_unrestricted() {
        let cwd_canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| {
            Path::new(cwd)
                .components()
                .filter(|c| !matches!(c, std::path::Component::CurDir))
                .collect()
        });
        if !canonical.starts_with(&cwd_canonical) {
            return Err(format!("cd: {} is outside the working directory", attempted.display()));
        }
    }
    Ok(canonical)
}

pub fn resolve_path(cwd: &str, path: &str) -> Result<std::path::PathBuf, String> {
    use std::path::{Component, Path, PathBuf};

    let cwd_norm: PathBuf = Path::new(cwd)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();

    let path_obj = Path::new(path);
    let normalized = if path_obj.is_absolute() {
        normalize_components(path_obj)
    } else {
        let joined = Path::new(cwd).join(path);
        normalize_components(&joined)
    };

    // TROGON_FS_UNRESTRICTED lets the typed tools operate on any path (matching bash's
    // freedom, à la Claude Code). Default: stay contained under cwd.
    if fs_unrestricted() {
        return Ok(std::fs::canonicalize(&normalized).unwrap_or(normalized));
    }

    if !normalized.starts_with(&cwd_norm) {
        return Err("path is outside the working directory".to_string());
    }

    let cwd_canonical = std::fs::canonicalize(cwd).unwrap_or(cwd_norm);

    // Resolve symlinks for parts that exist so a symlink pointing outside cwd
    // is caught. For paths that don't exist yet (new files), walk up to the
    // nearest existing ancestor, canonicalize it, and re-append the missing
    // trailing components before re-checking containment.
    if let Ok(canonical) = std::fs::canonicalize(&normalized) {
        if !canonical.starts_with(&cwd_canonical) {
            return Err("path is outside the working directory".to_string());
        }
        return Ok(canonical);
    }

    let mut ancestor = normalized.clone();
    let mut trailing: Vec<std::ffi::OsString> = Vec::new();
    while !ancestor.exists() {
        match ancestor.file_name() {
            Some(name) => trailing.push(name.to_os_string()),
            None => break,
        }
        if !ancestor.pop() {
            break;
        }
    }

    let mut resolved =
        std::fs::canonicalize(&ancestor).map_err(|_| "path is outside the working directory".to_string())?;
    trailing.reverse();
    for component in trailing {
        resolved.push(component);
    }

    if !resolved.starts_with(&cwd_canonical) {
        return Err("path is outside the working directory".to_string());
    }
    Ok(resolved)
}

fn normalize_components(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

/// Detected media type for a file read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Pdf,
}

impl MediaKind {
    fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Pdf => "application/pdf",
        }
    }
}

/// Sniff magic bytes to identify common image and PDF formats.
pub(crate) fn media_kind_from_bytes(data: &[u8]) -> Option<MediaKind> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(MediaKind::Png);
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return Some(MediaKind::Jpeg);
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some(MediaKind::Gif);
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some(MediaKind::Webp);
    }
    if data.starts_with(b"%PDF") {
        return Some(MediaKind::Pdf);
    }
    None
}

/// Map a file extension to a media kind (used when magic bytes are inconclusive).
pub(crate) fn media_kind_from_extension(path: &std::path::Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some(MediaKind::Png),
        "jpg" | "jpeg" => Some(MediaKind::Jpeg),
        "gif" => Some(MediaKind::Gif),
        "webp" => Some(MediaKind::Webp),
        "pdf" => Some(MediaKind::Pdf),
        _ => None,
    }
}

/// Resolve the media kind from file path and raw bytes (magic bytes take precedence).
pub(crate) fn detect_media_kind(path: &std::path::Path, data: &[u8]) -> Option<MediaKind> {
    media_kind_from_bytes(data).or_else(|| media_kind_from_extension(path))
}

fn image_output(data: &[u8], media_type: &str) -> ToolOutput {
    ToolOutput::Blocks(vec![ContentBlock::Image {
        source: ImageSource::Base64 {
            media_type: media_type.to_string(),
            data: BASE64.encode(data),
        },
    }])
}

fn extract_pdf_text(data: &[u8]) -> String {
    match pdf_extract::extract_text_from_mem(data) {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) => format!("PDF document ({} bytes). No extractable text found.", data.len()),
        Err(e) => format!("PDF document ({} bytes). Text extraction failed: {e}", data.len()),
    }
}

fn format_numbered_text(content: &str, offset: Option<usize>, limit: Option<usize>) -> Result<String, String> {
    let lines: Vec<&str> = content.lines().collect();
    let start = offset.unwrap_or(0);

    // MED-13: `offset == lines.len()` previously slipped through (`>` only) and
    // returned an empty string with no signal that the read was past the end.
    // An empty file (0 lines) at offset 0 is still a legitimate empty read.
    if start >= lines.len() && !lines.is_empty() {
        return Err(format!(
            "Error: offset {start} is at or past end of file ({} lines)",
            lines.len()
        ));
    }

    let end = limit.map(|l| (start + l).min(lines.len())).unwrap_or(lines.len());

    Ok(lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>6}\t{line}", start + i + 1))
        .collect::<Vec<_>>()
        .join("\n"))
}

pub async fn read_file(ctx: &ToolContext, input: &Value) -> ToolOutput {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ToolOutput::Text("Error: missing required parameter 'path'".to_string()),
    };
    let offset = input.get("offset").and_then(|v| v.as_u64()).map(|v| v as usize);
    let limit = input.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return ToolOutput::Text(format!("Error: {e}")),
    };

    let bytes = match tokio::fs::read(&full_path).await {
        Ok(b) => b,
        Err(e) => return ToolOutput::Text(format!("Error reading {path}: {e}")),
    };

    let kind = detect_media_kind(&full_path, &bytes);

    let mut output = match kind {
        Some(k @ (MediaKind::Png | MediaKind::Jpeg | MediaKind::Gif | MediaKind::Webp)) => {
            image_output(&bytes, k.mime_type())
        }
        Some(MediaKind::Pdf) => ToolOutput::Text(extract_pdf_text(&bytes)),
        None => {
            let content = match String::from_utf8(bytes) {
                Ok(c) => c,
                Err(_) => {
                    return ToolOutput::Text(format!("Error reading {path}: file is not valid UTF-8 text"));
                }
            };
            match format_numbered_text(&content, offset, limit) {
                Ok(text) => ToolOutput::Text(text),
                Err(e) => return ToolOutput::Text(e),
            }
        }
    };

    // On-demand TROGON.md: surface a subdirectory's rules when a file there is
    // read. The session-start loader walks UP from cwd, so cwd + ancestors are
    // already in the system prompt; subdirectories are not — this fills that gap.
    if kind.is_none()
        && let ToolOutput::Text(ref mut text) = output
        && let Some(extra) = subdir_trogon_md(&ctx.cwd, &full_path).await
    {
        text.push_str("\n\n");
        text.push_str(&extra);
    }
    output
}

/// When `file_path` lives in a directory *strictly below* `cwd`, return that
/// directory's `TROGON.md` content (if any), formatted for injection into the
/// read_file result. Files directly in `cwd` (or its ancestors) return `None`
/// because their TROGON.md is already loaded into the system prompt at session
/// start. Not deduplicated across calls: reading several files in the same
/// subdirectory re-surfaces its TROGON.md each time.
async fn subdir_trogon_md(cwd: &str, file_path: &std::path::Path) -> Option<String> {
    let cwd_canon = std::path::Path::new(cwd).canonicalize().ok()?;
    let dir_canon = file_path.parent()?.canonicalize().ok()?;
    if dir_canon == cwd_canon || !dir_canon.starts_with(&cwd_canon) {
        return None;
    }
    let md_path = dir_canon.join("TROGON.md");
    let content = tokio::fs::read_to_string(&md_path).await.ok()?;
    let content = content.trim();
    if content.is_empty() {
        return None;
    }
    Some(format!(
        "─── Directory rules: {}/TROGON.md ───\n{content}",
        dir_canon.display()
    ))
}

pub async fn write_file(ctx: &ToolContext, input: &Value) -> String {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'path'".to_string(),
    };
    let content = match input.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return "Error: missing required parameter 'content'".to_string(),
    };

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    if let Some(parent) = full_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        return format!("Error creating directories: {e}");
    }

    let tmp = unique_tmp_path(&full_path);
    if let Err(e) = tokio::fs::write(&tmp, content).await {
        return format!("Error writing file: {e}");
    }
    if let Err(e) = tokio::fs::rename(&tmp, &full_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return format!("Error saving file: {e}");
    }

    "OK".to_string()
}

pub async fn delete_file(ctx: &ToolContext, input: &Value) -> String {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'path'".to_string(),
    };

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    match tokio::fs::remove_file(&full_path).await {
        Ok(()) => "OK".to_string(),
        Err(e) => format!("Error deleting {path}: {e}"),
    }
}

pub async fn list_dir(ctx: &ToolContext, input: &Value) -> String {
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    let mut entries: Vec<String> = Vec::new();

    let walker = ignore::WalkBuilder::new(&full_path).hidden(false).build();

    for entry in walker {
        let e = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let rel = match e.path().strip_prefix(&full_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let depth = rel.components().count().saturating_sub(1);
        let indent = "  ".repeat(depth);
        let name = e.file_name().to_string_lossy();
        let suffix = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            "/"
        } else {
            ""
        };
        entries.push(format!("{indent}{name}{suffix}"));

        if entries.len() >= 500 {
            entries.push("... (truncated at 500 entries)".to_string());
            break;
        }
    }

    if entries.is_empty() {
        return "(empty directory)".to_string();
    }

    entries.join("\n")
}

pub async fn glob_files(ctx: &ToolContext, input: &Value) -> String {
    let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'pattern'".to_string(),
    };
    let base = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");

    let full_base = match resolve_path(&ctx.cwd, base) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    let matcher = match globset::Glob::new(pattern) {
        Ok(g) => g.compile_matcher(),
        Err(e) => return format!("Error: invalid glob pattern: {e}"),
    };

    const MAX_GLOB_RESULTS: usize = 500;
    let mut matches: Vec<String> = Vec::new();

    let walker = ignore::WalkBuilder::new(&full_base).hidden(false).build();

    for e in walker.flatten() {
        if matches.len() >= MAX_GLOB_RESULTS {
            matches.push(format!("… (truncated at {MAX_GLOB_RESULTS} results)"));
            break;
        }
        if e.file_type().map(|t| t.is_file()).unwrap_or(false)
            && let Ok(rel) = e.path().strip_prefix(&full_base)
            && matcher.is_match(rel)
        {
            matches.push(rel.to_string_lossy().into_owned());
        }
    }

    if matches.is_empty() {
        return "No files found".to_string();
    }

    matches.join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotebookEditMode {
    Replace,
    Insert,
    Delete,
}

fn parse_notebook_edit_mode(input: &Value) -> Result<NotebookEditMode, String> {
    if input.get("is_new_cell").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(NotebookEditMode::Insert);
    }
    match input.get("edit_mode").and_then(|v| v.as_str()) {
        None | Some("replace") => Ok(NotebookEditMode::Replace),
        Some("insert") => Ok(NotebookEditMode::Insert),
        Some("delete") => Ok(NotebookEditMode::Delete),
        Some(other) => Err(format!(
            "Error: invalid edit_mode '{other}' (expected 'replace', 'insert', or 'delete')"
        )),
    }
}

fn content_to_source_lines(content: &str) -> Vec<String> {
    let line_count = content.lines().count();
    content
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == line_count - 1 {
                line.to_string()
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

fn new_notebook_cell(cell_type: &str, content: &str) -> Result<serde_json::Value, String> {
    let source = content_to_source_lines(content);
    match cell_type {
        "code" => Ok(serde_json::json!({
            "cell_type": "code",
            "source": source,
            "metadata": {},
            "outputs": [],
            "execution_count": null
        })),
        "markdown" => Ok(serde_json::json!({
            "cell_type": "markdown",
            "source": source,
            "metadata": {}
        })),
        other => Err(format!(
            "Error: invalid cell_type '{other}' (expected 'code' or 'markdown')"
        )),
    }
}

pub async fn notebook_edit(ctx: &ToolContext, input: &Value) -> String {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'path'".to_string(),
    };
    let cell_index = match input.get("cell_index").and_then(|v| v.as_u64()) {
        Some(i) => i as usize,
        None => return "Error: missing required parameter 'cell_index'".to_string(),
    };
    let edit_mode = match parse_notebook_edit_mode(input) {
        Ok(mode) => mode,
        Err(e) => return e,
    };
    let content = input.get("content").and_then(|v| v.as_str());
    let cell_type = input.get("cell_type").and_then(|v| v.as_str());

    if edit_mode != NotebookEditMode::Delete && content.is_none() {
        return "Error: missing required parameter 'content'".to_string();
    }

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    let raw = match tokio::fs::read_to_string(&full_path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading notebook: {e}"),
    };

    let mut notebook: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return format!("Error parsing notebook JSON: {e}"),
    };

    let cells = match notebook.get_mut("cells").and_then(|v| v.as_array_mut()) {
        Some(c) => c,
        None => return "Error: notebook has no 'cells' array".to_string(),
    };

    match edit_mode {
        NotebookEditMode::Replace => {
            if cell_index >= cells.len() {
                return format!(
                    "Error: cell_index {cell_index} out of range (notebook has {} cells)",
                    cells.len()
                );
            }

            let content = content.expect("content checked above");
            let cell = &mut cells[cell_index];
            cell["source"] = serde_json::json!(content_to_source_lines(content));

            if let Some(ct) = cell_type {
                cell["cell_type"] = serde_json::json!(ct);
            }
        }
        NotebookEditMode::Insert => {
            if cell_index > cells.len() {
                return format!(
                    "Error: cell_index {cell_index} out of range for insert (notebook has {} cells)",
                    cells.len()
                );
            }

            let content = content.expect("content checked above");
            let ct = cell_type.unwrap_or("code");
            let new_cell = match new_notebook_cell(ct, content) {
                Ok(cell) => cell,
                Err(e) => return e,
            };
            cells.insert(cell_index, new_cell);
        }
        NotebookEditMode::Delete => {
            if cell_index >= cells.len() {
                return format!(
                    "Error: cell_index {cell_index} out of range (notebook has {} cells)",
                    cells.len()
                );
            }
            cells.remove(cell_index);
        }
    }

    let updated = match serde_json::to_string_pretty(&notebook) {
        Ok(s) => s,
        Err(e) => return format!("Error serializing notebook: {e}"),
    };

    if let Err(e) = tokio::fs::write(&full_path, updated).await {
        return format!("Error writing notebook: {e}");
    }

    "OK".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolOutput;
    use serde_json::json;
    use tempfile::TempDir;

    fn as_text(output: ToolOutput) -> String {
        output.display_text()
    }

    /// Smallest valid 1×1 PNG (67 bytes).
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00,
        0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18,
        0xdd, 0x8d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// Valid PDF fixture with extractable text "Hello PDF".
    const TEST_PDF: &[u8] = include_bytes!("../tests/fixtures/hello.pdf");

    fn ctx(dir: &TempDir) -> ToolContext {
        ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
            web_search_api_key: None,
            web_search_endpoint: None,
        }
    }

    #[test]
    fn resolve_path_normal_relative() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let result = resolve_path(&cwd, "src/main.rs").unwrap();
        assert_eq!(result, dir.path().join("src/main.rs"));
    }

    #[test]
    fn resolve_path_dot_prefix() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let result = resolve_path(&cwd, "./foo.txt").unwrap();
        assert_eq!(result, dir.path().join("foo.txt"));
    }

    #[test]
    fn resolve_path_dotdot_stays_within_cwd() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let result = resolve_path(&cwd, "a/../b.txt").unwrap();
        assert_eq!(result, dir.path().join("b.txt"));
    }

    #[test]
    fn resolve_path_dotdot_escapes_cwd_is_rejected() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let err = resolve_path(&cwd, "../../etc/passwd").unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[test]
    fn resolve_path_absolute_within_cwd_allowed() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let abs = dir.path().join("sub/file.txt").to_string_lossy().into_owned();
        let result = resolve_path(&cwd, &abs).unwrap();
        assert_eq!(result, dir.path().join("sub/file.txt"));
    }

    #[test]
    fn resolve_path_absolute_outside_cwd_rejected() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let err = resolve_path(&cwd, "/etc/passwd").unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_symlink_escaping_cwd_rejected() {
        // CRIT-2: a symlink inside cwd that points outside it must be rejected.
        // Lexical normalization alone passes the starts_with check; only
        // canonicalize catches the escape.
        let cwd_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();
        std::fs::write(outside_dir.path().join("secret.txt"), b"top secret").unwrap();
        std::os::unix::fs::symlink(outside_dir.path(), cwd_dir.path().join("evil")).unwrap();

        let cwd = cwd_dir.path().to_string_lossy().into_owned();
        let err = resolve_path(&cwd, "evil/secret.txt").unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_symlink_parent_new_file_rejected() {
        // NEW-5: a new file path through a symlinked parent dir that points
        // outside cwd must be rejected; lexical containment alone is not enough.
        let cwd_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();
        std::os::unix::fs::symlink(outside_dir.path(), cwd_dir.path().join("evil")).unwrap();

        let cwd = cwd_dir.path().to_string_lossy().into_owned();
        let err = resolve_path(&cwd, "evil/new_secret.txt").unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[test]
    fn resolve_path_new_file_directly_in_cwd_allowed() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let result = resolve_path(&cwd, "new_file.txt").unwrap();
        assert_eq!(result, dir.path().join("new_file.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_symlink_within_cwd_allowed() {
        // A symlink that stays inside cwd must still resolve successfully.
        let cwd_dir = TempDir::new().unwrap();
        std::fs::create_dir(cwd_dir.path().join("real")).unwrap();
        std::fs::write(cwd_dir.path().join("real/file.txt"), b"ok").unwrap();
        std::os::unix::fs::symlink(cwd_dir.path().join("real"), cwd_dir.path().join("link")).unwrap();

        let cwd = cwd_dir.path().to_string_lossy().into_owned();
        let resolved = resolve_path(&cwd, "link/file.txt").unwrap();
        // canonicalize() resolves the symlink to the real path inside cwd.
        assert!(resolved.starts_with(cwd_dir.path().canonicalize().unwrap()));
    }

    #[test]
    fn resolve_directory_target_empty_outside_cwd_rejected() {
        // B1: empty target resolves to $HOME, which is outside the sandbox cwd, so
        // it must be rejected rather than letting the model escape via `cd`.
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let err = resolve_directory_target(&cwd, "").unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[test]
    fn resolve_directory_target_relative() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let resolved = resolve_directory_target(&cwd, "sub").unwrap();
        assert_eq!(resolved, sub.canonicalize().unwrap());
    }

    #[test]
    fn resolve_directory_target_outside_cwd_rejected() {
        // B1: `cd /etc` (or any absolute path outside cwd) must be rejected.
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let err = resolve_directory_target(&cwd, "/etc").unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[test]
    fn resolve_directory_target_not_a_directory() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "").unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let err = resolve_directory_target(&cwd, "file.txt").unwrap_err();
        assert!(err.contains("not a directory"), "got: {err}");
    }

    #[test]
    fn resolve_directory_target_absolute_within_cwd() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let abs = sub.canonicalize().unwrap();
        let resolved = resolve_directory_target(&cwd, abs.to_str().unwrap()).unwrap();
        assert_eq!(resolved, abs);
    }

    #[test]
    fn media_kind_from_bytes_detects_png_jpeg_gif_webp_pdf() {
        assert_eq!(media_kind_from_bytes(TINY_PNG), Some(MediaKind::Png));
        assert_eq!(media_kind_from_bytes(b"\xff\xd8\xff\x00"), Some(MediaKind::Jpeg));
        assert_eq!(media_kind_from_bytes(b"GIF89a"), Some(MediaKind::Gif));
        assert_eq!(
            media_kind_from_bytes(b"RIFF\x00\x00\x00\x00WEBP"),
            Some(MediaKind::Webp)
        );
        assert_eq!(media_kind_from_bytes(TEST_PDF), Some(MediaKind::Pdf));
        assert_eq!(media_kind_from_bytes(b"plain text"), None);
    }

    #[test]
    fn media_kind_from_extension_maps_common_suffixes() {
        assert_eq!(
            media_kind_from_extension(std::path::Path::new("photo.JPG")),
            Some(MediaKind::Jpeg)
        );
        assert_eq!(
            media_kind_from_extension(std::path::Path::new("doc.pdf")),
            Some(MediaKind::Pdf)
        );
        assert_eq!(media_kind_from_extension(std::path::Path::new("readme.txt")), None);
    }

    #[test]
    fn extract_pdf_text_from_minimal_fixture() {
        let text = extract_pdf_text(TEST_PDF);
        assert!(text.contains("Hello PDF"), "expected extracted PDF text, got: {text}");
    }

    #[tokio::test]
    async fn read_file_png_returns_image_block() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("pixel.png"), TINY_PNG).await.unwrap();
        let ctx = ctx(&dir);
        let result = read_file(&ctx, &json!({"path": "pixel.png"})).await;
        match result {
            ToolOutput::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ContentBlock::Image {
                        source: ImageSource::Base64 { media_type, data },
                    } => {
                        assert_eq!(media_type, "image/png");
                        assert_eq!(data, &BASE64.encode(TINY_PNG));
                    }
                    other => panic!("expected Image block, got {other:?}"),
                }
            }
            other => panic!("expected Blocks output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_file_pdf_returns_extracted_text() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("doc.pdf"), TEST_PDF).await.unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "doc.pdf"})).await);
        assert!(
            result.contains("Hello PDF"),
            "expected PDF text extraction, got: {result}"
        );
    }

    #[tokio::test]
    async fn read_file_missing_path_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({})).await);
        assert!(result.contains("missing required parameter 'path'"), "got: {result}");
    }

    #[tokio::test]
    async fn read_file_offset_beyond_file_length_returns_error() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.txt"), "a\nb\nc").await.unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "f.txt", "offset": 100})).await);
        assert!(result.contains("Error"), "got: {result}");
        assert!(result.contains("past end"), "got: {result}");
    }

    #[tokio::test]
    async fn read_file_offset_equal_to_length_returns_error() {
        // MED-13: offset == line count must be reported, not silently empty.
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.txt"), "a\nb\nc").await.unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "f.txt", "offset": 3})).await);
        assert!(result.contains("past end"), "got: {result}");
    }

    #[tokio::test]
    async fn read_file_empty_file_at_offset_zero_is_ok() {
        // MED-13: an empty file (0 lines) read at offset 0 is a valid empty read.
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("empty.txt"), "").await.unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "empty.txt"})).await);
        assert!(!result.contains("Error"), "got: {result}");
        assert_eq!(result, "");
    }

    #[tokio::test]
    async fn read_file_returns_numbered_lines() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("hello.txt"), "line1\nline2\nline3")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "hello.txt"})).await);
        assert!(result.contains("1\tline1"));
        assert!(result.contains("2\tline2"));
        assert!(result.contains("3\tline3"));
    }

    #[tokio::test]
    async fn read_file_in_subdir_surfaces_directory_trogon_md() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("src");
        tokio::fs::create_dir_all(&sub).await.unwrap();
        tokio::fs::write(sub.join("main.rs"), "fn main() {}").await.unwrap();
        tokio::fs::write(sub.join("TROGON.md"), "Subdir rule: prefer iterators.")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "src/main.rs"})).await);
        assert!(result.contains("fn main()"), "file content missing: {result}");
        assert!(
            result.contains("Directory rules"),
            "subdir TROGON.md not surfaced: {result}"
        );
        assert!(
            result.contains("prefer iterators"),
            "subdir rule body missing: {result}"
        );
    }

    #[tokio::test]
    async fn read_file_in_cwd_does_not_resurface_cwd_trogon_md() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("TROGON.md"), "cwd rule")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "hello").await.unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "a.txt"})).await);
        assert!(result.contains("hello"));
        assert!(
            !result.contains("Directory rules"),
            "cwd TROGON.md must not be re-injected (already in system prompt): {result}"
        );
    }

    #[tokio::test]
    async fn read_file_with_offset_and_limit() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.txt"), "a\nb\nc\nd\ne")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "f.txt", "offset": 1, "limit": 2})).await);
        assert!(result.contains("2\tb"));
        assert!(result.contains("3\tc"));
        assert!(!result.contains("1\ta"));
        assert!(!result.contains("4\td"));
    }

    #[tokio::test]
    async fn read_file_missing_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "no_such_file.txt"})).await);
        assert!(result.starts_with("Error"));
    }

    #[tokio::test]
    async fn read_file_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = as_text(read_file(&ctx, &json!({"path": "../../etc/passwd"})).await);
        assert!(result.contains("Error"));
    }

    #[tokio::test]
    async fn write_file_missing_path_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = write_file(&ctx, &json!({"content": "x"})).await;
        assert!(result.contains("missing required parameter 'path'"), "got: {result}");
    }

    #[tokio::test]
    async fn write_file_missing_content_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = write_file(&ctx, &json!({"path": "f.txt"})).await;
        assert!(result.contains("missing required parameter 'content'"), "got: {result}");
    }

    #[tokio::test]
    async fn write_file_creates_and_reads_back() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = write_file(&ctx, &json!({"path": "out.txt", "content": "hello"})).await;
        assert_eq!(result, "OK");
        let content = tokio::fs::read_to_string(dir.path().join("out.txt")).await.unwrap();
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn write_file_creates_intermediate_dirs() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = write_file(&ctx, &json!({"path": "a/b/c.txt", "content": "deep"})).await;
        assert_eq!(result, "OK");
        assert!(dir.path().join("a/b/c.txt").exists());
    }

    #[tokio::test]
    async fn list_dir_returns_entries() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("foo.rs"), "").await.unwrap();
        tokio::fs::create_dir(dir.path().join("subdir")).await.unwrap();
        let ctx = ctx(&dir);
        let result = list_dir(&ctx, &json!({})).await;
        assert!(result.contains("foo.rs"));
        assert!(result.contains("subdir/"));
    }

    #[tokio::test]
    async fn list_dir_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = list_dir(&ctx, &json!({"path": "../../etc"})).await;
        assert!(result.contains("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn list_dir_empty_directory_returns_empty_message() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("empty");
        tokio::fs::create_dir(&inner).await.unwrap();
        let inner_ctx = ToolContext {
            proxy_url: String::new(),
            cwd: inner.to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
            web_search_api_key: None,
            web_search_endpoint: None,
        };
        let result = list_dir(&inner_ctx, &json!({})).await;
        assert_eq!(result, "(empty directory)");
    }

    #[tokio::test]
    async fn list_dir_nonexistent_path_returns_error_or_empty() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = list_dir(&ctx, &json!({"path": "no_such_dir"})).await;
        assert!(
            result == "(empty directory)" || result.starts_with("Error"),
            "got: {result}"
        );
    }

    #[tokio::test]
    async fn list_dir_truncates_at_500_entries() {
        let dir = TempDir::new().unwrap();
        for i in 0..510 {
            tokio::fs::write(dir.path().join(format!("file_{i:04}.txt")), "")
                .await
                .unwrap();
        }
        let ctx = ctx(&dir);
        let result = list_dir(&ctx, &json!({})).await;
        assert!(result.contains("truncated at 500 entries"), "got: {result}");
        let file_lines = result.lines().filter(|l| l.contains("file_")).count();
        assert_eq!(file_lines, 500, "expected exactly 500 file lines, got {file_lines}");
    }

    #[tokio::test]
    async fn glob_finds_matching_files() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("main.rs"), "").await.unwrap();
        tokio::fs::write(dir.path().join("lib.rs"), "").await.unwrap();
        tokio::fs::write(dir.path().join("readme.md"), "").await.unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({"pattern": "*.rs"})).await;
        assert!(result.contains("main.rs"));
        assert!(result.contains("lib.rs"));
        assert!(!result.contains("readme.md"));
    }

    #[tokio::test]
    async fn glob_path_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({"pattern": "*.rs", "path": "../../etc"})).await;
        assert!(result.contains("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn glob_missing_pattern_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({})).await;
        assert!(result.contains("missing required parameter 'pattern'"), "got: {result}");
    }

    #[tokio::test]
    async fn glob_no_matches_returns_no_files_found() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("readme.md"), "").await.unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({"pattern": "*.rs"})).await;
        assert_eq!(result, "No files found");
    }

    #[tokio::test]
    async fn glob_invalid_pattern_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({"pattern": "[invalid"})).await;
        assert!(result.starts_with("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn write_file_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = write_file(&ctx, &json!({"path": "../../evil.txt", "content": "bad"})).await;
        assert!(result.contains("Error"), "got: {result}");
        assert!(!std::path::Path::new("/tmp/evil.txt").exists());
    }

    #[tokio::test]
    async fn notebook_edit_missing_path_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(&ctx, &json!({"cell_index": 0, "content": "x"})).await;
        assert!(result.contains("missing required parameter 'path'"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_missing_cell_index_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(&ctx, &json!({"path": "nb.ipynb", "content": "x"})).await;
        assert!(
            result.contains("missing required parameter 'cell_index'"),
            "got: {result}"
        );
    }

    #[tokio::test]
    async fn notebook_edit_missing_content_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(&ctx, &json!({"path": "nb.ipynb", "cell_index": 0})).await;
        assert!(result.contains("missing required parameter 'content'"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "../../evil.ipynb", "cell_index": 0, "content": "x"}),
        )
        .await;
        assert!(result.contains("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_out_of_range_index_returns_error() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["x"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string_pretty(&nb).unwrap())
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(&ctx, &json!({"path": "nb.ipynb", "cell_index": 99, "content": "x"})).await;
        assert!(result.contains("Error"), "got: {result}");
        assert!(result.contains("out of range"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_nonexistent_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(&ctx, &json!({"path": "no_such.ipynb", "cell_index": 0, "content": "x"})).await;
        assert!(result.starts_with("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_updates_cell() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["old content"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string_pretty(&nb).unwrap())
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "nb.ipynb", "cell_index": 0, "content": "new content"}),
        )
        .await;
        assert_eq!(result, "OK");
        let raw = tokio::fs::read_to_string(dir.path().join("nb.ipynb")).await.unwrap();
        assert!(raw.contains("new content"));
    }

    #[tokio::test]
    async fn notebook_edit_changes_cell_type() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["x"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string_pretty(&nb).unwrap())
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "nb.ipynb", "cell_index": 0, "content": "# title", "cell_type": "markdown"}),
        )
        .await;
        assert_eq!(result, "OK");
        let raw = tokio::fs::read_to_string(dir.path().join("nb.ipynb")).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["cells"][0]["cell_type"], "markdown");
    }

    #[tokio::test]
    async fn notebook_edit_inserts_code_cell_at_index() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["first"], "metadata": {}, "outputs": [], "execution_count": null},
                {"cell_type": "code", "source": ["third"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string_pretty(&nb).unwrap())
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({
                "path": "nb.ipynb",
                "cell_index": 1,
                "content": "second",
                "cell_type": "code",
                "edit_mode": "insert"
            }),
        )
        .await;
        assert_eq!(result, "OK");
        let parsed: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(dir.path().join("nb.ipynb")).await.unwrap()).unwrap();
        assert_eq!(parsed["cells"].as_array().unwrap().len(), 3);
        assert_eq!(parsed["cells"][0]["source"][0], "first");
        assert_eq!(parsed["cells"][1]["source"][0], "second");
        assert_eq!(parsed["cells"][1]["cell_type"], "code");
        assert_eq!(parsed["cells"][2]["source"][0], "third");
    }

    #[tokio::test]
    async fn notebook_edit_inserts_markdown_cell_at_index() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["code"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string_pretty(&nb).unwrap())
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({
                "path": "nb.ipynb",
                "cell_index": 0,
                "content": "# heading",
                "cell_type": "markdown",
                "edit_mode": "insert"
            }),
        )
        .await;
        assert_eq!(result, "OK");
        let parsed: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(dir.path().join("nb.ipynb")).await.unwrap()).unwrap();
        assert_eq!(parsed["cells"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["cells"][0]["cell_type"], "markdown");
        assert_eq!(parsed["cells"][0]["source"][0], "# heading");
        assert_eq!(parsed["cells"][1]["source"][0], "code");
    }

    #[tokio::test]
    async fn notebook_edit_insert_via_is_new_cell() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["only"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string_pretty(&nb).unwrap())
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({
                "path": "nb.ipynb",
                "cell_index": 1,
                "content": "appended",
                "cell_type": "code",
                "is_new_cell": true
            }),
        )
        .await;
        assert_eq!(result, "OK");
        let parsed: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(dir.path().join("nb.ipynb")).await.unwrap()).unwrap();
        assert_eq!(parsed["cells"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["cells"][0]["source"][0], "only");
        assert_eq!(parsed["cells"][1]["source"][0], "appended");
    }

    #[tokio::test]
    async fn notebook_edit_delete_cell() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["keep"], "metadata": {}, "outputs": [], "execution_count": null},
                {"cell_type": "code", "source": ["remove"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string_pretty(&nb).unwrap())
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "nb.ipynb", "cell_index": 1, "edit_mode": "delete"}),
        )
        .await;
        assert_eq!(result, "OK");
        let parsed: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(dir.path().join("nb.ipynb")).await.unwrap()).unwrap();
        assert_eq!(parsed["cells"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["cells"][0]["source"][0], "keep");
    }
}
