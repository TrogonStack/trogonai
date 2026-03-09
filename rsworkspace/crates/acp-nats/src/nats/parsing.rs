use super::client_method::ClientMethod;
use super::parsed_client_subject::ParsedClientSubject;
use crate::session_id::AcpSessionId;

pub fn parse_client_subject(subject: &str) -> Option<ParsedClientSubject> {
    let client_byte_pos = subject.rmatch_indices(".client.").next()?.0;

    let before_client = &subject[..client_byte_pos];
    let session_dot = before_client.rfind('.')?;
    let session_id = &before_client[session_dot + 1..];
    let session_id = AcpSessionId::new(session_id).ok()?;

    let suffix = &subject[client_byte_pos + 1..];

    ClientMethod::from_subject_suffix(suffix)
        .map(|method| ParsedClientSubject { session_id, method })
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
    }

    #[test]
    fn test_parse_unknown_method() {
        assert!(parse_client_subject("acp.sess123.client.unknown.method").is_none());
    }

    #[test]
    fn test_parse_rejects_invalid_session_tokens() {
        assert!(parse_client_subject("acp.session*id.client.session.update").is_none());
    }

    #[test]
    fn test_parse_with_client_in_prefix() {
        let subject = "org.client.app.sess123.client.session.update";
        let parsed = parse_client_subject(subject).unwrap();
        assert_eq!(parsed.session_id.as_str(), "sess123");
        assert_eq!(parsed.method, ClientMethod::SessionUpdate);
    }
}
