#[derive(clap::Subcommand, Clone, Copy, Debug)]
pub enum Operation {
    /// Create missing ACP streams and reconcile their owned subject filters.
    Provision,
}
