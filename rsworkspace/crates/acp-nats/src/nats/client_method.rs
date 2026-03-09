#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMethod {
    FsReadTextFile,
    SessionRequestPermission,
    SessionUpdate,
    TerminalCreate,
    TerminalKill,
    TerminalOutput,
}

impl ClientMethod {
    pub fn from_subject_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "client.fs.read_text_file" => Some(Self::FsReadTextFile),
            "client.session.request_permission" => Some(Self::SessionRequestPermission),
            "client.session.update" => Some(Self::SessionUpdate),
            "client.terminal.create" => Some(Self::TerminalCreate),
            "client.terminal.kill" => Some(Self::TerminalKill),
            "client.terminal.output" => Some(Self::TerminalOutput),
            _ => None,
        }
    }
}
