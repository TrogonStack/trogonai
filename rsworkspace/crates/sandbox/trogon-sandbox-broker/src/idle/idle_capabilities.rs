/// What the lease's provider can do for the idle chain.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct IdleCapabilities {
    /// The provider supports pause.
    pub pause: bool,
    /// The provider can suspend this lease (it offers a suspend level and the
    /// lease allows the artifact it leaves behind).
    pub suspend: bool,
}
