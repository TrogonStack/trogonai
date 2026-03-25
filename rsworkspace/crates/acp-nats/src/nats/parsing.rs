use crate::ext_method_name::ExtMethodName;
use crate::session_id::AcpSessionId;

const AGENT_MARKER: &str = ".agent.";
const AGENT_EXT_PREFIX: &str = "agent.ext.";

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
    pub fn is_session_scoped(&self) -> bool {
        !matches!(
            self,
            Self::Initialize
                | Self::Authenticate
                | Self::SessionNew
                | Self::SessionList
                | Self::Ext(_)
        )
    }

    fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "agent.initialize" => Some(Self::Initialize),
            "agent.authenticate" => Some(Self::Authenticate),
            "agent.session.new" => Some(Self::SessionNew),
            "agent.session.list" => Some(Self::SessionList),
            "agent.session.load" => Some(Self::SessionLoad),
            "agent.session.prompt" => Some(Self::SessionPrompt),
            "agent.session.cancel" => Some(Self::SessionCancel),
            "agent.session.set_mode" => Some(Self::SessionSetMode),
            "agent.session.set_config_option" => Some(Self::SessionSetConfigOption),
            "agent.session.set_model" => Some(Self::SessionSetModel),
            "agent.session.fork" => Some(Self::SessionFork),
            "agent.session.resume" => Some(Self::SessionResume),
            "agent.session.close" => Some(Self::SessionClose),
            other => {
                let ext_name = other.strip_prefix(AGENT_EXT_PREFIX)?;
                Some(Self::Ext(ExtMethodName::new(ext_name).ok()?))
            }
        }
    }
}

#[derive(Debug)]
pub struct ParsedAgentSubject {
    pub session_id: Option<AcpSessionId>,
    pub method: AgentMethod,
}

pub fn parse_agent_subject(subject: &str) -> Option<ParsedAgentSubject> {
    for (agent_byte_pos, _) in subject.match_indices(AGENT_MARKER) {
        let suffix = &subject[agent_byte_pos + 1..];
        let method = match AgentMethod::from_suffix(suffix) {
            Some(m) => m,
            None => continue,
        };

        let session_id = if method.is_session_scoped() {
            let before_agent = &subject[..agent_byte_pos];
            let session_dot = before_agent.rfind('.')?;
            let raw = &before_agent[session_dot + 1..];
            Some(AcpSessionId::new(raw).ok()?)
        } else {
            None
        };

        return Some(ParsedAgentSubject { session_id, method });
    }

    None
}

/// NATS subject prefix for generic extension methods.
/// `client.ext.{name}` — the `ext` token makes extensions explicit in subjects.
const EXT_SUBJECT_PREFIX: &str = "client.ext.";

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
    Ext(String),
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
    let client_byte_pos = subject.rmatch_indices(".client.").next()?.0;

    let before_client = &subject[..client_byte_pos];
    let session_dot = before_client.rfind('.')?;
    let session_id = &before_client[session_dot + 1..];
    let session_id = AcpSessionId::new(session_id).ok()?;

    let suffix = &subject[client_byte_pos + 1..];

    let method = ClientMethod::from_subject_suffix(suffix)?;

    Some(ParsedClientSubject { session_id, method })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fs_read_text_file() {
        let subject = "acp.sess123.client.fs.read_text_file";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
    }

    #[test]
    fn test_parse_fs_write_text_file() {
        let subject = "acp.sess456.client.fs.write_text_file";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess456");
        assert_eq!(parsed.method, ClientMethod::FsWriteTextFile);
    }

    #[test]
    fn test_parse_session_request_permission() {
        let subject = "acp.sess123.client.session.request_permission";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::SessionRequestPermission);
    }

    #[test]
    fn test_parse_session_update() {
        let subject = "acp.sess123.client.session.update";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::SessionUpdate);
    }

    #[test]
    fn test_parse_terminal_create() {
        let subject = "acp.sess123.client.terminal.create";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::TerminalCreate);
    }

    #[test]
    fn test_parse_terminal_kill() {
        let subject = "acp.sess123.client.terminal.kill";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::TerminalKill);
    }

    #[test]
    fn test_parse_terminal_output() {
        let subject = "acp.sess123.client.terminal.output";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::TerminalOutput);
    }

    #[test]
    fn test_parse_terminal_release() {
        let subject = "acp.sess123.client.terminal.release";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::TerminalRelease);
    }

    #[test]
    fn test_parse_terminal_wait_for_exit() {
        let subject = "acp.sess123.client.terminal.wait_for_exit";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::TerminalWaitForExit);
    }

    #[test]
    fn test_parse_with_custom_prefix() {
        let subject = "myapp.sess123.client.session.update";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::SessionUpdate);
    }

    #[test]
    fn test_parse_with_long_prefix() {
        let subject = "my.multi.part.prefix.sess123.client.session.update";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::SessionUpdate);
    }

    #[test]
    fn test_parse_empty_subject() {
        assert!(parse_client_subject("").is_none());
    }

    #[test]
    fn test_parse_no_method_suffix() {
        assert!(parse_client_subject("acp.client").is_none());
        assert!(parse_client_subject("acp.sess.client").is_none());
    }

    #[test]
    fn test_parse_no_client_marker() {
        assert!(parse_client_subject("acp.sess123.agent.initialize").is_none());
        assert!(parse_client_subject("acp.sess123.other.method").is_none());
    }

    #[test]
    fn test_parse_unknown_method() {
        assert!(parse_client_subject("acp.sess123.client.unknown.method").is_none());
    }

    #[test]
    fn test_parse_rejects_invalid_session_tokens() {
        assert!(parse_client_subject("acp.session*id.client.session.update").is_none());
        assert!(parse_client_subject("acp.session id.client.fs.read_text_file").is_none());
        assert!(parse_client_subject("acp.session>id.client.fs.read_text_file").is_none());
    }

    #[test]
    fn test_parse_with_client_in_prefix() {
        let subject = "org.client.app.sess123.client.session.update";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::SessionUpdate);
    }

    #[test]
    fn test_parse_malformed_structure() {
        assert!(parse_client_subject("...").is_none());
        assert!(parse_client_subject("acp..client.method").is_none());
    }

    #[test]
    fn test_parse_empty_session_id() {
        assert!(parse_client_subject("acp..client.fs.read_text_file").is_none());
    }

    #[test]
    fn test_parse_client_not_in_correct_position() {
        assert!(parse_client_subject("client.acp.sess123.method").is_none());
    }

    #[test]
    fn test_client_method_from_suffix_all_variants() {
        let test_cases = vec![
            (
                "client.fs.read_text_file",
                Some(ClientMethod::FsReadTextFile),
            ),
            (
                "client.fs.write_text_file",
                Some(ClientMethod::FsWriteTextFile),
            ),
            (
                "client.session.request_permission",
                Some(ClientMethod::SessionRequestPermission),
            ),
            ("client.session.update", Some(ClientMethod::SessionUpdate)),
            ("client.terminal.create", Some(ClientMethod::TerminalCreate)),
            ("client.terminal.kill", Some(ClientMethod::TerminalKill)),
            ("client.terminal.output", Some(ClientMethod::TerminalOutput)),
            (
                "client.terminal.release",
                Some(ClientMethod::TerminalRelease),
            ),
            (
                "client.terminal.wait_for_exit",
                Some(ClientMethod::TerminalWaitForExit),
            ),
            (
                "client.ext.my_method",
                Some(ClientMethod::Ext("my_method".to_string())),
            ),
            (
                "client.ext.vendor.operation",
                Some(ClientMethod::Ext("vendor.operation".to_string())),
            ),
            ("client.unknown", None),
            ("client.ext.", None),
            ("client.ext.method.*", None),
            ("", None),
        ];

        for (suffix, expected) in test_cases {
            assert_eq!(
                ClientMethod::from_subject_suffix(suffix),
                expected,
                "Failed for suffix: {}",
                suffix
            );
        }
    }

    #[test]
    fn test_parse_session_id_extraction() {
        let test_cases = vec![
            ("acp.sess1.client.fs.read_text_file", "sess1"),
            (
                "myapp.my-session-123.client.terminal.create",
                "my-session-123",
            ),
            (
                "prefix.session_with_underscores.client.session.update",
                "session_with_underscores",
            ),
        ];

        for (subject, expected_session_id) in test_cases {
            let parsed = parse_client_subject(subject).unwrap();
            assert_eq!(parsed.session_id.as_str(), expected_session_id);
        }
    }

    #[test]
    fn test_parse_ext_method() {
        let subject = "acp.sess123.client.ext.my_custom_method";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(
            parsed.method,
            ClientMethod::Ext("my_custom_method".to_string())
        );
    }

    #[test]
    fn test_parse_ext_method_dotted_namespace() {
        let subject = "acp.sess123.client.ext.vendor.operation";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(
            parsed.method,
            ClientMethod::Ext("vendor.operation".to_string())
        );
    }

    #[test]
    fn test_parse_ext_empty_name_returns_none() {
        assert!(parse_client_subject("acp.sess123.client.ext.").is_none());
    }

    #[test]
    fn test_parse_ext_wildcard_name_returns_none() {
        assert!(parse_client_subject("acp.sess123.client.ext.method.*").is_none());
    }

    #[test]
    fn test_parse_ext_rejects_ambiguous_client_in_name() {
        assert!(parse_client_subject("acp.sess123.client.ext.foo.client.bar").is_none());
    }

    #[test]
    fn test_parse_ext_with_client_in_prefix() {
        let subject = "org.client.app.sess123.client.ext.my_tool";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::Ext("my_tool".to_string()));
    }

    #[test]
    fn test_client_method_equality() {
        assert_eq!(ClientMethod::FsReadTextFile, ClientMethod::FsReadTextFile);
        assert_ne!(ClientMethod::FsReadTextFile, ClientMethod::FsWriteTextFile);
    }

    #[test]
    fn test_agent_parse_initialize() {
        let parsed = parse_agent_subject("acp.agent.initialize").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Initialize);
    }

    #[test]
    fn test_agent_parse_authenticate() {
        let parsed = parse_agent_subject("acp.agent.authenticate").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Authenticate);
    }

    #[test]
    fn test_agent_parse_session_new() {
        let parsed = parse_agent_subject("acp.agent.session.new").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::SessionNew);
    }

    #[test]
    fn test_agent_parse_session_list() {
        let parsed = parse_agent_subject("acp.agent.session.list").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::SessionList);
    }

    #[test]
    fn test_agent_parse_session_load() {
        let parsed = parse_agent_subject("acp.s1.agent.session.load").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionLoad);
    }

    #[test]
    fn test_agent_parse_session_prompt() {
        let parsed = parse_agent_subject("acp.s1.agent.session.prompt").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionPrompt);
    }

    #[test]
    fn test_agent_parse_session_cancel() {
        let parsed = parse_agent_subject("acp.s1.agent.session.cancel").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionCancel);
    }

    #[test]
    fn test_agent_parse_session_set_mode() {
        let parsed = parse_agent_subject("acp.s1.agent.session.set_mode").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionSetMode);
    }

    #[test]
    fn test_agent_parse_session_set_config_option() {
        let parsed = parse_agent_subject("acp.s1.agent.session.set_config_option").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionSetConfigOption);
    }

    #[test]
    fn test_agent_parse_session_set_model() {
        let parsed = parse_agent_subject("acp.s1.agent.session.set_model").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionSetModel);
    }

    #[test]
    fn test_agent_parse_session_fork() {
        let parsed = parse_agent_subject("acp.s1.agent.session.fork").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionFork);
    }

    #[test]
    fn test_agent_parse_session_resume() {
        let parsed = parse_agent_subject("acp.s1.agent.session.resume").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionResume);
    }

    #[test]
    fn test_agent_parse_session_close() {
        let parsed = parse_agent_subject("acp.s1.agent.session.close").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionClose);
    }

    #[test]
    fn test_agent_parse_ext_method() {
        let parsed = parse_agent_subject("acp.agent.ext.my_tool").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(
            parsed.method,
            AgentMethod::Ext(ExtMethodName::new("my_tool").unwrap())
        );
    }

    #[test]
    fn test_agent_parse_ext_dotted_namespace() {
        let parsed = parse_agent_subject("acp.agent.ext.vendor.operation").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(
            parsed.method,
            AgentMethod::Ext(ExtMethodName::new("vendor.operation").unwrap())
        );
    }

    #[test]
    fn test_agent_parse_custom_prefix() {
        let parsed = parse_agent_subject("myapp.agent.initialize").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Initialize);
    }

    #[test]
    fn test_agent_parse_multi_part_prefix() {
        let parsed = parse_agent_subject("my.multi.prefix.s1.agent.session.load").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionLoad);
    }

    #[test]
    fn test_agent_parse_empty_returns_none() {
        assert!(parse_agent_subject("").is_none());
    }

    #[test]
    fn test_agent_parse_no_agent_marker_returns_none() {
        assert!(parse_agent_subject("acp.client.session.update").is_none());
    }

    #[test]
    fn test_agent_parse_unknown_method_returns_none() {
        assert!(parse_agent_subject("acp.agent.unknown.method").is_none());
    }

    #[test]
    fn test_agent_parse_invalid_session_id_returns_none() {
        assert!(parse_agent_subject("acp.sess*ion.agent.session.load").is_none());
    }

    #[test]
    fn test_agent_parse_ext_empty_name_returns_none() {
        assert!(parse_agent_subject("acp.agent.ext.").is_none());
    }

    #[test]
    fn test_agent_parse_ext_wildcard_returns_none() {
        assert!(parse_agent_subject("acp.agent.ext.*").is_none());
    }

    #[test]
    fn test_agent_parse_multi_dot_prefix_global_method_has_no_session() {
        let parsed = parse_agent_subject("my.multi.agent.initialize").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Initialize);
    }

    #[test]
    fn test_agent_parse_prefix_containing_agent_word() {
        let parsed = parse_agent_subject("org.agent.app.agent.initialize").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(parsed.method, AgentMethod::Initialize);
    }

    #[test]
    fn test_agent_parse_ext_method_containing_agent_segment() {
        let parsed = parse_agent_subject("acp.agent.ext.agent.foo").unwrap();
        assert!(parsed.session_id.is_none());
        assert_eq!(
            parsed.method,
            AgentMethod::Ext(ExtMethodName::new("agent.foo").unwrap())
        );
    }

    #[test]
    fn test_agent_parse_multi_dot_prefix_session_scoped() {
        let parsed = parse_agent_subject("my.multi.s1.agent.session.prompt").unwrap();
        assert_eq!(parsed.session_id.unwrap().as_str(), "s1");
        assert_eq!(parsed.method, AgentMethod::SessionPrompt);
    }
}
