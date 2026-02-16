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

pub struct ParsedClientSubject {
    pub session_id: String,
    pub method: ClientMethod,
}

pub fn parse_client_subject(subject: &str) -> Option<ParsedClientSubject> {
    let parts: Vec<&str> = subject.split('.').collect();

    // Format: <prefix>.<session_id>.client.<method>
    // We need at least 4 parts: prefix, session_id, "client", and method
    if parts.len() < 4 {
        return None;
    }

    // Find "client" marker - it should appear after prefix and session_id
    let client_index = parts.iter().position(|&p| p == "client")?;

    // Need at least prefix and session_id before "client"
    if client_index < 2 {
        return None;
    }

    // Session ID is the part immediately before "client"
    let session_id = parts[client_index - 1].to_string();

    // Everything from "client" onwards is the suffix
    let suffix = parts[client_index..].join(".");

    ClientMethod::from_subject_suffix(&suffix)
        .map(|method| ParsedClientSubject { session_id, method })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Happy path tests
    #[test]
    fn test_parse_fs_read_text_file() {
        let subject = "acp.sess123.client.fs.read_text_file";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id, "sess123");
        assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
    }

    #[test]
    fn test_parse_fs_write_text_file() {
        let subject = "acp.sess456.client.fs.write_text_file";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id, "sess456");
        assert_eq!(parsed.method, ClientMethod::FsWriteTextFile);
    }

    #[test]
    fn test_parse_terminal_create() {
        let subject = "acp.sess789.client.terminal.create";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id, "sess789");
        assert_eq!(parsed.method, ClientMethod::TerminalCreate);
    }

    #[test]
    fn test_parse_session_request_permission() {
        let subject = "acp.sess_abc.client.session.request_permission";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id, "sess_abc");
        assert_eq!(parsed.method, ClientMethod::SessionRequestPermission);
    }

    #[test]
    fn test_parse_ext_session_prompt_response() {
        let subject = "acp.sess999.client.ext.session.prompt_response";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id, "sess999");
        assert_eq!(parsed.method, ClientMethod::ExtSessionPromptResponse);
    }

    #[test]
    fn test_parse_with_custom_prefix() {
        let subject = "myapp.sess123.client.fs.read_text_file";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id, "sess123");
        assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
    }

    #[test]
    fn test_parse_with_long_prefix() {
        let subject = "my.multi.part.prefix.sess123.client.terminal.output";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id, "sess123");
        assert_eq!(parsed.method, ClientMethod::TerminalOutput);
    }

    // Error cases - deterministic failure testing
    #[test]
    fn test_parse_empty_subject() {
        assert!(parse_client_subject("").is_none());
    }

    #[test]
    fn test_parse_too_short() {
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
    fn test_parse_malformed_structure() {
        assert!(parse_client_subject("...").is_none());
        assert!(parse_client_subject("acp..client.method").is_none());
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
            assert_eq!(parsed.session_id, expected_session_id);
        }
    }

    #[test]
    fn test_client_method_equality() {
        assert_eq!(ClientMethod::FsReadTextFile, ClientMethod::FsReadTextFile);
        assert_ne!(ClientMethod::FsReadTextFile, ClientMethod::FsWriteTextFile);
    }
}
