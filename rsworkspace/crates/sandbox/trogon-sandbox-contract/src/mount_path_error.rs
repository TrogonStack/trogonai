/// A drive mount path is not a safe absolute directory.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum MountPathError {
    #[error("mount path must be absolute")]
    NotAbsolute,
    #[error("mount path must not resolve to '/'")]
    Root,
    #[error("mount path must not contain '..' components")]
    ParentComponent,
}
