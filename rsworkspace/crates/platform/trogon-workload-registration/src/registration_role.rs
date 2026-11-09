#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationRole {
    Provisioner,
    Publisher,
    Revoker,
}

impl RegistrationRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Provisioner => "provisioner",
            Self::Publisher => "publisher",
            Self::Revoker => "revoker",
        }
    }
}
