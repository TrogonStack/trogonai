#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConformanceCase {
    KeySubstitution,
    BindingSubstitution,
    Tampering,
}
