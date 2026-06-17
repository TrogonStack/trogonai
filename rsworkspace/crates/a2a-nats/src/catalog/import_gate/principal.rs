pub use a2a_identity_types::SpiceDbPrincipal;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_account_name_round_trip() {
        let n = ImportedAccountName::new("peer-acct");
        assert_eq!(n.as_str(), "peer-acct");
        assert_eq!(n.to_string(), "peer-acct");
    }
}
