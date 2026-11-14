/// Platform names `getPlatform()` upstream returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlatformKind {
    Macos,
    Windows,
    Wsl,
    Linux,
    Unknown,
}

impl PlatformKind {
    /// The name `getPlatform()` upstream returns.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Wsl => "wsl",
            Self::Linux => "linux",
            Self::Unknown => "unknown",
        }
    }
}
