use super::*;

fn domain() -> TrustDomain {
    TrustDomain::new("trogon.example").expect("trust domain")
}

#[test]
fn round_trips_every_agent_id() {
    for raw in ["a", "agent-1", "A_b.c-9"] {
        let agent = AgentId::new(raw).expect("agent");
        let uri = agent_uri(&agent, &domain()).expect("alias");
        assert_eq!(agent_id_from_uri(&uri, &domain()), Ok(agent));
    }
}

#[test]
fn alias_has_the_documented_shape() {
    let agent = AgentId::new("agent-1").expect("agent");
    assert_eq!(
        agent_uri(&agent, &domain()).as_deref(),
        Ok("spiffe://trogon.example/agent/agent-1")
    );
}

#[test]
fn rejects_other_schemes_and_casings() {
    for uri in [
        "https://trogon.example/agent/a",
        "SPIFFE://trogon.example/agent/a",
        "spiffe:/trogon.example/agent/a",
        "trogon.example/agent/a",
        "",
    ] {
        assert_eq!(
            agent_id_from_uri(uri, &domain()),
            Err(AgentUriError::NotSpiffe),
            "{uri:?}"
        );
    }
}

#[test]
fn rejects_other_trust_domains_and_authority_decorations() {
    for uri in [
        "spiffe://other.example/agent/a",
        "spiffe://trogon.example:443/agent/a",
        "spiffe://user@trogon.example/agent/a",
        "spiffe://TROGON.example/agent/a",
        "spiffe://trogon.example.evil/agent/a",
        "spiffe:///agent/a",
    ] {
        assert_eq!(
            agent_id_from_uri(uri, &domain()),
            Err(AgentUriError::WrongTrustDomain),
            "{uri:?}"
        );
    }
}

#[test]
fn rejects_wrong_paths() {
    for uri in [
        "spiffe://trogon.example",
        "spiffe://trogon.example/",
        "spiffe://trogon.example/agents/a",
        "spiffe://trogon.example/workload/a",
        "spiffe://trogon.example//agent/a",
        "spiffe://trogon.example/agent",
    ] {
        assert_eq!(
            agent_id_from_uri(uri, &domain()),
            Err(AgentUriError::WrongPath),
            "{uri:?}"
        );
    }
}

#[test]
fn rejects_malformed_agent_segments() {
    for uri in [
        "spiffe://trogon.example/agent/",
        "spiffe://trogon.example/agent/a/",
        "spiffe://trogon.example/agent/a/b",
        "spiffe://trogon.example/agent/a?x=1",
        "spiffe://trogon.example/agent/a#frag",
        "spiffe://trogon.example/agent/%61",
        "spiffe://trogon.example/agent/a b",
    ] {
        assert!(
            matches!(agent_id_from_uri(uri, &domain()), Err(AgentUriError::InvalidAgentId(_))),
            "{uri:?}"
        );
    }
}

#[test]
fn rejects_dot_segments_and_empty_or_trailing_segments() {
    for uri in [
        "spiffe://trogon.example/agent/.",
        "spiffe://trogon.example/agent/..",
        "spiffe://trogon.example/agent/./",
        "spiffe://trogon.example/agent/../a",
        "spiffe://trogon.example/agent//a",
        "spiffe://trogon.example/agent/a//",
    ] {
        assert!(
            matches!(agent_id_from_uri(uri, &domain()), Err(AgentUriError::InvalidAgentId(_))),
            "{uri:?}"
        );
    }
}

/// `AgentId` is wider than a SPIFFE path segment (a policy document may key a
/// `did:key:` id), so the SPIFFE boundary is strict in both directions.
const DID: &str = "did:key:z6MkhaXgBZDvotDkL5257faiztiGi2sfKfzvuU5TVndZqquw";

#[test]
fn an_agent_id_outside_the_spiffe_path_alphabet_has_no_alias() {
    let agent = AgentId::new(DID).expect("a general agent id");
    assert_eq!(agent_uri(&agent, &domain()), Err(AgentUriError::NotSpiffeConformant));
}

#[test]
fn a_certificate_uri_outside_the_spiffe_path_alphabet_names_no_agent() {
    for segment in [DID, "a:b", "a:"] {
        let uri = format!("spiffe://trogon.example/agent/{segment}");
        assert_eq!(
            agent_id_from_uri(&uri, &domain()),
            Err(AgentUriError::NotSpiffeConformant),
            "{uri:?}"
        );
    }
}

#[test]
fn every_rendered_alias_is_a_valid_spiffe_id() {
    for raw in ["a", "agent-1", "A_b.c-9", "...", ".a", "a."] {
        let agent = AgentId::new(raw).expect("agent");
        let uri = agent_uri(&agent, &domain()).expect("alias");
        assert!(spiffe::SpiffeId::new(&uri).is_ok(), "{uri:?}");
    }
}
