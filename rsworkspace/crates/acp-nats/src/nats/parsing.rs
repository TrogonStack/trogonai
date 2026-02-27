use crate::session_id::AcpSessionId;

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

#[derive(Debug)]
pub struct ParsedClientSubject {
    pub session_id: AcpSessionId,
    pub method: ClientMethod,
}

pub fn parse_client_subject(subject: &str) -> Option<ParsedClientSubject> {
    // Find "client" marker scanning from the end so that a prefix
    // containing "client" as a token (e.g. "org.client.app") is skipped.
    let client_byte_pos = subject.rmatch_indices(".client.").next()?.0;

    // Everything before ".client." must contain at least prefix.session_id
    let before_client = &subject[..client_byte_pos];
    let session_dot = before_client.rfind('.')?;
    let session_id = &before_client[session_dot + 1..];
    // Session ID is expected to be a single NATS token. AcpSessionId enforces
    // this (including rejecting dots), which is required for the suffix
    // matching logic below to be unambiguous.
    let session_id = AcpSessionId::new(session_id).ok()?;

    // Everything from "client" onwards is the suffix
    let suffix = &subject[client_byte_pos + 1..];

    ClientMethod::from_subject_suffix(suffix).map(|method| ParsedClientSubject {
        session_id,
        method,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Happy path tests
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
    fn test_parse_terminal_create() {
        let subject = "acp.sess789.client.terminal.create";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess789");
        assert_eq!(parsed.method, ClientMethod::TerminalCreate);
    }

    #[test]
    fn test_parse_session_request_permission() {
        let subject = "acp.sess_abc.client.session.request_permission";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess_abc");
        assert_eq!(parsed.method, ClientMethod::SessionRequestPermission);
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
        let subject = "myapp.sess123.client.fs.read_text_file";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
    }

    #[test]
    fn test_parse_with_long_prefix() {
        let subject = "my.multi.part.prefix.sess123.client.terminal.output";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::TerminalOutput);
    }

    // Error cases - deterministic failure testing
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
        assert!(parse_client_subject("acp.session*id.client.fs.read_text_file").is_none());
        assert!(parse_client_subject("acp.session id.client.fs.read_text_file").is_none());
        assert!(parse_client_subject("acp.session>id.client.fs.read_text_file").is_none());
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

    // Test all ClientMethod variants
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
            ("client.unknown", None),
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
    fn test_parse_with_client_in_prefix() {
        let subject = "org.client.app.sess123.client.fs.read_text_file";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
    }

    #[test]
    fn test_client_method_equality() {
        assert_eq!(ClientMethod::FsReadTextFile, ClientMethod::FsReadTextFile);
        assert_ne!(ClientMethod::FsReadTextFile, ClientMethod::FsWriteTextFile);
    }
}
