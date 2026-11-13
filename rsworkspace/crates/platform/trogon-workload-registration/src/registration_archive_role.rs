#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationArchiveRole {
    Provisioner,
    Writer,
    Reader,
}
impl RegistrationArchiveRole {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::Provisioner => "provisioner",
            Self::Writer => "writer",
            Self::Reader => "reader",
        }
    }
}
