#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMethod {
    FsReadTextFile,
    FsWriteTextFile,
    SessionRequestPermission,
    SessionUpdate,
    TerminalCreate,
    TerminalKill,
    TerminalOutput,
    TerminalRelease,
    TerminalWaitForExit,
    ExtSessionPromptResponse,
}

impl ClientMethod {
    pub fn from_subject_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "client.fs.read_text_file" => Some(Self::FsReadTextFile),
            "client.fs.write_text_file" => Some(Self::FsWriteTextFile),
            "client.session.request_permission" => Some(Self::SessionRequestPermission),
            "client.session.update" => Some(Self::SessionUpdate),
            "client.terminal.create" => Some(Self::TerminalCreate),
            "client.terminal.kill" => Some(Self::TerminalKill),
            "client.terminal.output" => Some(Self::TerminalOutput),
            "client.terminal.release" => Some(Self::TerminalRelease),
            "client.terminal.wait_for_exit" => Some(Self::TerminalWaitForExit),
            "client.ext.session.prompt_response" => Some(Self::ExtSessionPromptResponse),
            _ => None,
        }
    }
}
