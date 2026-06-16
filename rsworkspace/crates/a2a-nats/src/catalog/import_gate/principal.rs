pub use a2a_auth_callout::SpiceDbPrincipal;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImportedAccountName(String);

impl ImportedAccountName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ImportedAccountName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
