    use super::*;

    #[test]
    fn accepts_dotted_subject() {
        let subject = NatsPushSubject::new("a2a.push.acme.caller-42.task-9").unwrap();
        assert_eq!(subject.as_str(), "a2a.push.acme.caller-42.task-9");
    }

    #[test]
    fn rejects_empty() {
        assert!(NatsPushSubject::new("").is_err());
    }

    #[test]
    fn rejects_wildcards() {
        assert!(NatsPushSubject::new("a2a.push.*").is_err());
    }

    #[test]
    fn wire_form_prefixes_subject() {
        let subject = NatsPushSubject::new("a2a.push.t.caller.task").unwrap();
        assert_eq!(subject.wire_form(), "subject:a2a.push.t.caller.task");
    }

    #[test]
    fn error_display_includes_violation() {
        let err = NatsPushSubject::new("").unwrap_err();
        assert!(err.to_string().contains("invalid NATS push subject"));
    }

    #[test]
    fn display_renders_inner_token() {
        let subject = NatsPushSubject::new("a2a.push.bot.caller.task").unwrap();
        assert_eq!(subject.to_string(), "a2a.push.bot.caller.task");
    }
