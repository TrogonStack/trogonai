/// A project directory that [`ProjectDir`](crate::ProjectDir) refuses.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ProjectDirError {
    #[error("project dir must be an absolute path, got \"{path}\"")]
    NotAbsolute { path: String },
}
