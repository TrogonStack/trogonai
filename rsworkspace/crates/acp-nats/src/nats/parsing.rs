use crate::ext_method_name::ExtMethodName;
use crate::session_id::AcpSessionId;

/// NATS subject prefix for generic extension methods.
/// `client.ext.{name}` — the `ext` token makes extensions explicit in subjects.
/// `ExtSessionPromptResponse` is matched first as a specific ext, so it won't
/// collide with this catch-all.
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
    ExtSessionPromptResponse,
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
            "client.ext.session.prompt_response" => Some(Self::ExtSessionPromptResponse),
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

    if let ClientMethod::Ext(ref name) = method
        && name.contains(".client.")
    {
        return None;
    }

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
    fn test_parse_ext_session_prompt_response() {
        let subject = "acp.sess999.client.ext.session.prompt_response";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess999");
        assert_eq!(parsed.method, ClientMethod::ExtSessionPromptResponse);
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
                "client.ext.session.prompt_response",
                Some(ClientMethod::ExtSessionPromptResponse),
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
    fn test_parse_ext_does_not_shadow_prompt_response() {
        let subject = "acp.sess123.client.ext.session.prompt_response";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.method, ClientMethod::ExtSessionPromptResponse);
    }

    #[test]
    fn test_client_method_equality() {
        assert_eq!(ClientMethod::FsReadTextFile, ClientMethod::FsReadTextFile);
        assert_ne!(ClientMethod::FsReadTextFile, ClientMethod::FsWriteTextFile);
    }
}
