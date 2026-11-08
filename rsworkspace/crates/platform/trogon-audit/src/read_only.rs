/// Whether a surface has declared a method read-only in its own contract
/// (ADR#0086 Decision 3).
///
/// The default is [`ReadOnly::Undeclared`], so a surface that says nothing is
/// audited durably. The egress proxy declares none, because a passthrough tunnel
/// is opaque and cannot tell a read from a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReadOnly {
    Declared,
    #[default]
    Undeclared,
}
