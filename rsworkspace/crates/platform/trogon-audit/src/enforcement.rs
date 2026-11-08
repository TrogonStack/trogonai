/// Whether a layer's verdict is enforced or only observed (ADR#0084 DryRun).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Enforcement {
    Enforce,
    DryRun,
}
