use clap::ValueEnum;

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum IsolationBackendKind {
    #[cfg(target_os = "linux")]
    Bubblewrap,
    #[cfg(target_os = "macos")]
    Seatbelt,
    /// The sandbox account of the Windows network fence, started by the fence helper's `exec` subcommand.
    #[cfg(windows)]
    Fenced,
    #[value(name = "none")]
    None,
}
