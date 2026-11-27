use trogon_sandbox_contract::FileOpErrorKind;

/// The failure kind a guest tool's last stderr clause names.
pub(super) fn stderr_kind(stderr: &str) -> FileOpErrorKind {
    let detail = stderr.rsplit(": ").next().unwrap_or(stderr).to_lowercase();
    if detail.contains("permission denied") || detail.contains("read-only") {
        FileOpErrorKind::PermissionDenied
    } else if detail.contains("not a directory") {
        FileOpErrorKind::NotADirectory
    } else if detail.contains("is a directory") {
        FileOpErrorKind::IsADirectory
    } else if detail.contains("not empty") {
        FileOpErrorKind::DirectoryNotEmpty
    } else if detail.contains("file exists") {
        FileOpErrorKind::AlreadyExists
    } else {
        FileOpErrorKind::Io
    }
}
