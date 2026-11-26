use acp_nats::AcpPrefix;
use acp_nats::nats::{GlobalAgentMethod, ParsedAgentSubject, global, parse_agent_subject};
use acp_nats::wire::{decode_request_params, encode_request};
use agent_client_protocol::v1::schema::AuthStatusRequest;
use jsonrpc_nats::RequestId;

#[test]
fn auth_status_keeps_the_subject_projection_distinct_from_the_body_method() -> Result<(), Box<dyn std::error::Error>> {
    for prefix in ["acp", "tenant.blue"] {
        let prefix = AcpPrefix::new(prefix)?;
        let subject = global::AuthStatusSubject::new(&prefix).to_string();
        let Some(ParsedAgentSubject::Global(method)) = parse_agent_subject(&subject) else {
            return Err("auth status subject must parse as a global agent method".into());
        };

        assert_eq!(method, GlobalAgentMethod::AuthStatus);
        assert_eq!(method.wire_method(), "auth.status");
        assert_eq!(method.protocol_method(), "auth/status");
        assert_eq!(
            subject,
            format!("{}.v1.global.agent.{}", prefix.as_str(), method.wire_method())
        );

        let request = AuthStatusRequest::new();
        let encoded = encode_request(&method.protocol_method(), RequestId::Number(7), &request)?;
        let decoded: AuthStatusRequest = decode_request_params("auth/status", &encoded.headers, &encoded.body)?;
        assert_eq!(decoded, request);
        assert!(
            decode_request_params::<AuthStatusRequest>(&method.wire_method(), &encoded.headers, &encoded.body).is_err(),
            "the NATS terminal must not be accepted as the ACP body method"
        );
    }
    Ok(())
}

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
