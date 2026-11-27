/// A set of fields is not a [`WasiPreopenSpec`](crate::WasiPreopenSpec) the wire accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum WasiPreopenSpecError {
    #[error("host_dir must not be empty")]
    EmptyHostDir,
    #[error("guest_path must be absolute")]
    RelativeGuestPath,
    #[error("guest_path must not contain '..' components")]
    ParentComponent,
}
