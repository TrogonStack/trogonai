/// A `read_file` target that does not exist.
///
/// Workers return it from their read path and the verb handler maps it to `ReadFileResponse::NotFound`, the reply
/// every worker gives for a missing file.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("file not found: {path}")]
pub struct FileNotFoundError {
    pub path: String,
}
