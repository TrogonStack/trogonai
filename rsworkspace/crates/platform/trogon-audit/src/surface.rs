#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    A2aIngress,
    Egress,
    /// The secrets service deciding a credential release.
    SecretsRelease,
}
