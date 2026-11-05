use rcgen::string::Ia5String;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};

use super::*;

fn domain() -> TrustDomain {
    TrustDomain::new("trogon.example").expect("trust domain")
}

fn certificate(sans: Vec<SanType>, common_name: Option<&str>) -> Vec<u8> {
    let mut params = CertificateParams::default();
    params.subject_alt_names = sans;
    params.distinguished_name = DistinguishedName::new();
    if let Some(common_name) = common_name {
        params.distinguished_name.push(DnType::CommonName, common_name);
    }
    let key = KeyPair::generate().expect("key");
    params.self_signed(&key).expect("certificate").der().to_vec()
}

fn uri(value: &str) -> SanType {
    SanType::URI(Ia5String::try_from(value).expect("ia5"))
}

fn dns(value: &str) -> SanType {
    SanType::DnsName(Ia5String::try_from(value).expect("ia5"))
}

#[test]
fn takes_the_agent_from_the_single_uri_san() {
    let der = certificate(vec![uri("spiffe://trogon.example/agent/agent-7")], None);
    assert_eq!(
        agent_id_from_certificate(&der, &domain()),
        Ok(AgentId::new("agent-7").expect("agent"))
    );
}

#[test]
fn other_san_kinds_do_not_count_as_uris() {
    let der = certificate(
        vec![dns("agent-9.example"), uri("spiffe://trogon.example/agent/agent-7")],
        None,
    );
    assert_eq!(
        agent_id_from_certificate(&der, &domain()),
        Ok(AgentId::new("agent-7").expect("agent"))
    );
}

#[test]
fn rejects_two_uri_sans_even_when_both_name_the_same_agent() {
    let der = certificate(
        vec![
            uri("spiffe://trogon.example/agent/agent-7"),
            uri("spiffe://trogon.example/agent/agent-7"),
        ],
        None,
    );
    assert_eq!(
        agent_id_from_certificate(&der, &domain()),
        Err(PeerIdentityError::MultipleUriSans)
    );
}

#[test]
fn rejects_a_certificate_without_a_uri_san() {
    let none = certificate(vec![], None);
    assert_eq!(
        agent_id_from_certificate(&none, &domain()),
        Err(PeerIdentityError::NoUriSan)
    );
    let dns_only = certificate(vec![dns("agent-7.example")], None);
    assert_eq!(
        agent_id_from_certificate(&dns_only, &domain()),
        Err(PeerIdentityError::NoUriSan)
    );
}

#[test]
fn never_falls_back_to_the_subject_dn() {
    let der = certificate(vec![], Some("spiffe://trogon.example/agent/agent-7"));
    assert_eq!(
        agent_id_from_certificate(&der, &domain()),
        Err(PeerIdentityError::NoUriSan)
    );
    let der = certificate(vec![dns("x.example")], Some("agent-7"));
    assert_eq!(
        agent_id_from_certificate(&der, &domain()),
        Err(PeerIdentityError::NoUriSan)
    );
}

#[test]
fn rejects_a_uri_that_does_not_map_to_an_agent() {
    let wrong_domain = certificate(vec![uri("spiffe://other.example/agent/agent-7")], None);
    assert_eq!(
        agent_id_from_certificate(&wrong_domain, &domain()),
        Err(PeerIdentityError::Uri(AgentUriError::WrongTrustDomain))
    );
    let https = certificate(vec![uri("https://trogon.example/agent/agent-7")], None);
    assert_eq!(
        agent_id_from_certificate(&https, &domain()),
        Err(PeerIdentityError::Uri(AgentUriError::NotSpiffe))
    );
}

#[test]
fn rejects_garbage_and_trailing_bytes() {
    assert_eq!(
        agent_id_from_certificate(b"not a certificate", &domain()),
        Err(PeerIdentityError::Unparsable)
    );
    assert_eq!(
        agent_id_from_certificate(&[], &domain()),
        Err(PeerIdentityError::Unparsable)
    );
    let mut der = certificate(vec![uri("spiffe://trogon.example/agent/agent-7")], None);
    der.push(0);
    assert_eq!(
        agent_id_from_certificate(&der, &domain()),
        Err(PeerIdentityError::Unparsable)
    );
}
