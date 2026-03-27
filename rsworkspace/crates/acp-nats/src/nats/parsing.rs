use crate::constants::{
    EXT_SUBJECT_PREFIX, SESSION_AGENT_MARKER, SESSION_CLIENT_MARKER, SESSION_PREFIX,
};
use crate::ext_method_name::ExtMethodName;
use crate::session_id::AcpSessionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMethod {
    Initialize,
    Authenticate,
    SessionNew,
    SessionList,
    SessionLoad,
    SessionPrompt,
    SessionCancel,
    SessionSetMode,
    SessionSetConfigOption,
    SessionSetModel,
    SessionFork,
    SessionResume,
    SessionClose,
    Ext(ExtMethodName),
}

impl AgentMethod {
    fn from_global_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "initialize" => Some(Self::Initialize),
            "authenticate" => Some(Self::Authenticate),
            "session.new" => Some(Self::SessionNew),
            "session.list" => Some(Self::SessionList),
            other => {
                let ext_name = other.strip_prefix("ext.")?;
                Some(Self::Ext(ExtMethodName::new(ext_name).ok()?))
            }
        }
    }

    fn from_session_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "load" => Some(Self::SessionLoad),
            "prompt" => Some(Self::SessionPrompt),
            "cancel" => Some(Self::SessionCancel),
            "set_mode" => Some(Self::SessionSetMode),
            "set_config_option" => Some(Self::SessionSetConfigOption),
            "set_model" => Some(Self::SessionSetModel),
            "fork" => Some(Self::SessionFork),
            "resume" => Some(Self::SessionResume),
            "close" => Some(Self::SessionClose),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct ParsedAgentSubject {
    pub session_id: Option<AcpSessionId>,
    pub method: AgentMethod,
}

pub fn parse_agent_subject(subject: &str) -> Option<ParsedAgentSubject> {
    if let Some(parsed) = try_parse_session_agent(subject) {
        return Some(parsed);
    }

    let agent_pos = subject.rfind(".agent.")?;
    let suffix = &subject[agent_pos + ".agent.".len()..];
    let method = AgentMethod::from_global_suffix(suffix)?;
    Some(ParsedAgentSubject {
        session_id: None,
        method,
    })
}

fn try_parse_session_agent(subject: &str) -> Option<ParsedAgentSubject> {
    for (session_pos, _) in subject.match_indices(SESSION_PREFIX) {
        let after_session = &subject[session_pos + SESSION_PREFIX.len()..];
        if let Some(agent_pos) = after_session.find(SESSION_AGENT_MARKER) {
            let sid = &after_session[..agent_pos];
            if let Ok(session_id) = AcpSessionId::new(sid) {
                let method_suffix = &after_session[agent_pos + SESSION_AGENT_MARKER.len()..];
                if let Some(method) = AgentMethod::from_session_suffix(method_suffix) {
                    return Some(ParsedAgentSubject {
                        session_id: Some(session_id),
                        method,
                    });
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
mod tests {
    use super::*;

    // Agent global methods

    #[test]
    fn parse_agent_initialize() {
        let parsed = parse_agent_subject("acp.agent.initialize").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Initialize);
    }

    #[test]
    fn parse_agent_authenticate() {
        let parsed = parse_agent_subject("acp.agent.authenticate").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Authenticate);
    }

    #[test]
    fn parse_agent_session_new() {
        let parsed = parse_agent_subject("acp.agent.session.new").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::SessionNew);
    }

    #[test]
    fn parse_agent_session_list() {
        let parsed = parse_agent_subject("acp.agent.session.list").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::SessionList);
    }

    #[test]
    fn parse_agent_ext() {
        let parsed = parse_agent_subject("acp.agent.ext.my_tool").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(
            parsed.method,
            AgentMethod::Ext(ExtMethodName::new("my_tool").unwrap())
        );
    }

    #[test]
    fn parse_agent_ext_dotted() {
        let parsed = parse_agent_subject("acp.agent.ext.vendor.op").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(
            parsed.method,
            AgentMethod::Ext(ExtMethodName::new("vendor.op").unwrap())
        );
    }

    // Session-scoped agent methods

    #[test]
    fn parse_session_agent_load() {
        let parsed = parse_agent_subject("acp.session.s1.agent.load").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionLoad);
    }

    #[test]
    fn parse_session_agent_prompt() {
        let parsed = parse_agent_subject("acp.session.s1.agent.prompt").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionPrompt);
    }

    #[test]
    fn parse_session_agent_cancel() {
        let parsed = parse_agent_subject("acp.session.s1.agent.cancel").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionCancel);
    }

    #[test]
    fn parse_session_agent_set_mode() {
        let parsed = parse_agent_subject("acp.session.s1.agent.set_mode").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionSetMode);
    }

    #[test]
    fn parse_session_agent_set_config_option() {
        let parsed = parse_agent_subject("acp.session.s1.agent.set_config_option").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionSetConfigOption);
    }

    #[test]
    fn parse_session_agent_set_model() {
        let parsed = parse_agent_subject("acp.session.s1.agent.set_model").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionSetModel);
    }

    #[test]
    fn parse_session_agent_fork() {
        let parsed = parse_agent_subject("acp.session.s1.agent.fork").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionFork);
    }

    #[test]
    fn parse_session_agent_resume() {
        let parsed = parse_agent_subject("acp.session.s1.agent.resume").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionResume);
    }

    #[test]
    fn parse_session_agent_close() {
        let parsed = parse_agent_subject("acp.session.s1.agent.close").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionClose);
    }

    #[test]
    fn parse_agent_custom_prefix() {
        let parsed = parse_agent_subject("myapp.agent.initialize").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Initialize);
    }

    #[test]
    fn parse_session_agent_custom_prefix() {
        let parsed = parse_agent_subject("myapp.session.s1.agent.prompt").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionPrompt);
    }

    #[test]
    fn parse_agent_empty_returns_none() {
        assert!(parse_agent_subject("").is_none());
    }

    #[test]
    fn parse_agent_unknown_method_returns_none() {
        assert!(parse_agent_subject("acp.agent.unknown").is_none());
    }

    #[test]
    fn parse_agent_invalid_session_id_returns_none() {
        assert!(parse_agent_subject("acp.session.s*1.agent.load").is_none());
    }

    #[test]
    fn parse_agent_client_subject_returns_none() {
        assert!(parse_agent_subject("acp.session.s1.client.session.update").is_none());
    }

    #[test]
    fn parse_agent_no_overlap_with_session_id_agent() {
        let parsed = parse_agent_subject("acp.session.agent.agent.load").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "agent");
        assert_eq!(parsed.method, AgentMethod::SessionLoad);
    }

    // Client methods

    #[test]
    fn parse_client_fs_read() {
        let parsed = parse_client_subject("acp.session.s1.client.fs.read_text_file").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
    }

    #[test]
    fn parse_client_fs_write() {
        let parsed = parse_client_subject("acp.session.s1.client.fs.write_text_file").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::FsWriteTextFile);
    }

    #[test]
    fn parse_client_request_permission() {
        let parsed =
            parse_client_subject("acp.session.s1.client.session.request_permission").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::SessionRequestPermission);
    }

    #[test]
    fn parse_client_session_update() {
        let parsed = parse_client_subject("acp.session.s1.client.session.update").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::SessionUpdate);
    }

    #[test]
    fn parse_client_terminal_create() {
        let parsed = parse_client_subject("acp.session.s1.client.terminal.create").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::TerminalCreate);
    }

    #[test]
    fn parse_client_terminal_kill() {
        let parsed = parse_client_subject("acp.session.s1.client.terminal.kill").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::TerminalKill);
    }

    #[test]
    fn parse_client_terminal_output() {
        let parsed = parse_client_subject("acp.session.s1.client.terminal.output").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::TerminalOutput);
    }

    #[test]
    fn parse_client_terminal_release() {
        let parsed = parse_client_subject("acp.session.s1.client.terminal.release").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::TerminalRelease);
    }

    #[test]
    fn parse_client_terminal_wait() {
        let parsed = parse_client_subject("acp.session.s1.client.terminal.wait_for_exit").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::TerminalWaitForExit);
    }

    #[test]
    fn parse_client_ext_prompt_response() {
        let parsed =
            parse_client_subject("acp.session.s1.client.ext.session.prompt_response").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::ExtSessionPromptResponse);
    }

    #[test]
    fn parse_client_custom_prefix() {
        let parsed = parse_client_subject("myapp.session.s1.client.fs.read_text_file").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
    }

    #[test]
    fn parse_client_empty_returns_none() {
        assert!(parse_client_subject("").is_none());
    }

    #[test]
    fn parse_client_no_session_returns_none() {
        assert!(parse_client_subject("acp.client.fs.read_text_file").is_none());
    }

    #[test]
    fn parse_client_unknown_method_returns_none() {
        assert!(parse_client_subject("acp.session.s1.client.unknown").is_none());
    }

    #[test]
    fn parse_client_session_but_no_valid_method_returns_none() {
        assert!(parse_client_subject("acp.session.s1.client.nope").is_none());
    }

    #[test]
    fn parse_client_prefix_containing_session_word() {
        let parsed =
            parse_client_subject("my.session.app.session.s1.client.fs.read_text_file").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
    }

    #[test]
    fn parse_session_agent_unknown_method_returns_none() {
        assert!(parse_agent_subject("acp.session.s1.agent.unknown").is_none());
    }

    #[test]
    fn parse_agent_prefix_containing_agent_word() {
        let parsed = parse_agent_subject("org.agent.app.agent.initialize").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Initialize);
    }

    #[test]
    fn parse_session_agent_prefix_containing_session_word() {
        let parsed = parse_agent_subject("my.session.app.session.s1.agent.prompt").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionPrompt);
    }

    #[test]
    fn parse_agent_prefix_containing_session_falls_through_to_global() {
        let parsed = parse_agent_subject("my.session.handler.agent.initialize").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Initialize);
    }

    #[test]
    fn parse_client_ext_method() {
        let parsed = parse_client_subject("acp.session.s1.client.ext.my_tool").unwrap();
        assert_eq!(parsed.session_id.as_str(), "s1");
        assert_eq!(parsed.method, ClientMethod::Ext("my_tool".to_string()));
    }
}
