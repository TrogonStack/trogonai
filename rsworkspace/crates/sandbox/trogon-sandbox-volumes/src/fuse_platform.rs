/// Whether FUSE mounts can work on the running platform.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FusePlatform {
    Supported,
    Unsupported,
}

impl FusePlatform {
    /// FUSE mounts are available on Linux only.
    #[cfg(target_os = "linux")]
    pub const fn current() -> Self {
        Self::Supported
    }

    /// FUSE mounts are available on Linux only.
    #[cfg(not(target_os = "linux"))]
    pub const fn current() -> Self {
        Self::Unsupported
    }
}
