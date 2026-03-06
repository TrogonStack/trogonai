#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMethod {
    FsReadTextFile,
    SessionUpdate,
}

impl ClientMethod {
    pub fn from_subject_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "client.fs.read_text_file" => Some(Self::FsReadTextFile),
            "client.session.update" => Some(Self::SessionUpdate),
            _ => None,
        }
    }
}
