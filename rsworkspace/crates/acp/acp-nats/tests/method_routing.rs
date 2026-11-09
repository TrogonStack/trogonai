use acp_nats::nats::{ParsedAgentSubject, parse_agent_subject};

#[test]
fn schema_1_15_methods_have_canonical_nats_routes() {
    let cases = [
        ("acp.v1.global.agent.auth.status", "auth/status"),
        ("acp.v1.session.s1.agent.status", "session/status"),
        ("acp.v1.session.s1.agent.set_title", "session/set_title"),
        ("acp.v1.session.s1.agent.archive", "session/archive"),
        ("acp.v1.session.s1.agent.unarchive", "session/unarchive"),
    ];
    let missing: Vec<_> = cases
        .into_iter()
        .filter_map(|(subject, expected)| {
            let actual = match parse_agent_subject(subject) {
                Some(ParsedAgentSubject::Global(method)) => Some(method.protocol_method()),
                Some(ParsedAgentSubject::Session { method, .. }) => Some(method.protocol_method().to_owned()),
                None => None,
            };
            (actual.as_deref() != Some(expected)).then_some((subject, expected, actual))
        })
        .collect();
    assert!(missing.is_empty(), "unrouted schema methods: {missing:?}");
}
