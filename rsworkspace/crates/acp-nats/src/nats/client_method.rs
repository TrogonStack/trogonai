#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMethod {
    SessionUpdate,
}

impl ClientMethod {
    pub fn from_subject_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "client.session.update" => Some(Self::SessionUpdate),
            _ => None,
        }
    }
}
