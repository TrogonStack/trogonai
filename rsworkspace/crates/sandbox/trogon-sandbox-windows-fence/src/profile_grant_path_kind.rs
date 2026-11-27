/// How the grant treats one recorded path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileGrantPathKind {
    /// Listable, children granted one by one.
    Container,
    /// A directory readable with everything below it.
    Granted,
    /// A single readable file.
    File,
    /// An exclusion.
    Denied,
}
