use crate::constants::{EXT_SUBJECT_PREFIX, SESSION_AGENT_MARKER, SESSION_CLIENT_MARKER, SESSION_PREFIX};
use crate::ext_method_name::ExtMethodName;
use crate::session_id::AcpSessionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalAgentMethod {
    Initialize,
    Authenticate,
    Logout,
    SessionNew,
    SessionList,
    Ext(ExtMethodName),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAgentMethod {
    Load,
    Prompt,
    Cancel,
    SetMode,
    SetConfigOption,
    SetModel,
    Fork,
    Resume,
    Close,
}

impl GlobalAgentMethod {
    fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "initialize" => Some(Self::Initialize),
            "authenticate" => Some(Self::Authenticate),
            "logout" => Some(Self::Logout),
            "session.new" => Some(Self::SessionNew),
            "session.list" => Some(Self::SessionList),
            other => {
                let ext_name = other.strip_prefix("ext.")?;
                Some(Self::Ext(ExtMethodName::new(ext_name).ok()?))
            }
        }
    }
}

impl SessionAgentMethod {
    fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "load" => Some(Self::Load),
            "prompt" => Some(Self::Prompt),
            "cancel" => Some(Self::Cancel),
            "set_mode" => Some(Self::SetMode),
            "set_config_option" => Some(Self::SetConfigOption),
            "set_model" => Some(Self::SetModel),
            "fork" => Some(Self::Fork),
            "resume" => Some(Self::Resume),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParsedAgentSubject {
    Global(GlobalAgentMethod),
    Session {
        session_id: AcpSessionId,
        method: SessionAgentMethod,
    },
}

pub fn parse_agent_subject(subject: &str) -> Option<ParsedAgentSubject> {
    if let Some(parsed) = try_parse_session_agent(subject) {
        return Some(parsed);
    }

    let agent_pos = subject.rfind(".agent.")?;
    let suffix = &subject[agent_pos + ".agent.".len()..];
    let method = GlobalAgentMethod::from_suffix(suffix)?;
    Some(ParsedAgentSubject::Global(method))
}

fn try_parse_session_agent(subject: &str) -> Option<ParsedAgentSubject> {
    for (session_pos, _) in subject.match_indices(SESSION_PREFIX) {
        let after_session = &subject[session_pos + SESSION_PREFIX.len()..];
        if let Some(agent_pos) = after_session.find(SESSION_AGENT_MARKER) {
            let sid = &after_session[..agent_pos];
            if let Ok(session_id) = AcpSessionId::new(sid) {
                let method_suffix = &after_session[agent_pos + SESSION_AGENT_MARKER.len()..];
                if let Some(method) = SessionAgentMethod::from_suffix(method_suffix) {
                    return Some(ParsedAgentSubject::Session { session_id, method });
                }
            }
        }
    }
    None
}

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
    Ext(String),
}

impl ClientMethod {
    pub fn from_subject_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "fs.read_text_file" => Some(Self::FsReadTextFile),
            "fs.write_text_file" => Some(Self::FsWriteTextFile),
            "session.request_permission" => Some(Self::SessionRequestPermission),
            "session.update" => Some(Self::SessionUpdate),
            "terminal.create" => Some(Self::TerminalCreate),
            "terminal.kill" => Some(Self::TerminalKill),
            "terminal.output" => Some(Self::TerminalOutput),
            "terminal.release" => Some(Self::TerminalRelease),
            "terminal.wait_for_exit" => Some(Self::TerminalWaitForExit),
            "ext.session.prompt_response" => Some(Self::ExtSessionPromptResponse),
            other => {
                let ext_name = other.strip_prefix(EXT_SUBJECT_PREFIX)?;
                ExtMethodName::new(ext_name).ok()?;
                Some(Self::Ext(ext_name.to_string()))
            }
        }
    }
}

#[derive(Debug)]
pub struct ParsedClientSubject {
    pub session_id: AcpSessionId,
    pub method: ClientMethod,
}

pub fn parse_client_subject(subject: &str) -> Option<ParsedClientSubject> {
    for (session_pos, _) in subject.match_indices(SESSION_PREFIX) {
        let after_session = &subject[session_pos + SESSION_PREFIX.len()..];
        if let Some(client_pos) = after_session.find(SESSION_CLIENT_MARKER) {
            let sid = &after_session[..client_pos];
            if let Ok(session_id) = AcpSessionId::new(sid) {
                let method_suffix = &after_session[client_pos + SESSION_CLIENT_MARKER.len()..];
                if let Some(method) = ClientMethod::from_subject_suffix(method_suffix) {
                    return Some(ParsedClientSubject { session_id, method });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;
