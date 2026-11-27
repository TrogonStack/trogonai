/// Whether a missing `ipfilter` or `macfilter` on the guest NIC fails the lease or only logs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum AntiSpoof {
    /// Refuse to boot a guest whose NIC filters could not be verified.
    #[default]
    Require,
    /// Log a warning and boot anyway.
    Warn,
}
