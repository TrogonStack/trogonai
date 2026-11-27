/// Why a software bill of materials could not be generated.
#[derive(Debug, thiserror::Error)]
pub enum SyftError {
    #[error("syft binary not found")]
    NotInstalled,
    #[error("syft failed: {0}")]
    Failed(String),
}
