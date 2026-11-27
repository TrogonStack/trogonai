use super::*;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncWriteExt, duplex};
use tokio::sync::RwLock;
use trogon_sandbox_contract::{AddressDenialClass, AllowRange, NetworkOptions, Port};
use trogon_sandbox_contract::{
    DecideRequest, DomainRule, HttpMethod, IpRange, LeaseId, NamesMatchedByName, NetworkPolicy,
};

use super::super::IngressLimits;
use super::super::token::ProxyToken;
use crate::decider::{
    Decider, DeciderLimits, DeciderTransport, DeciderTransportError, LivePolicyAmender, PolicyPublisher,
};
use crate::held_writes::{HeldWrites, WAIT, expect_bytes};
use crate::resolve::{NameLookup, SsrfFloor, VettingResolver};

#[tokio::test]
async fn bytes_written_ahead_of_a_splice_are_flushed_upstream_first() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream) = duplex(64 * 1024);
    let splice = tokio::spawn(super::splice_after(
        proxy_client,
        HeldWrites::new(proxy_upstream),
        b"\x16\x03\x01hello",
    ));
    expect_bytes(&mut upstream, b"\x16\x03\x01hello", "the lead bytes").await;
    client.shutdown().await?;
    upstream.shutdown().await?;
    let outcome = tokio::time::timeout(WAIT, splice).await??;
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }));
    Ok(())
}

#[test]
fn first_byte_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(Protocol::from_first_byte(0x05), Some(Protocol::Socks5));
    assert_eq!(Protocol::from_first_byte(0x04), Some(Protocol::Socks4));
    for method in *b"CGPDHO\r\n" {
        assert_eq!(Protocol::from_first_byte(method), Some(Protocol::Http));
    }
    for other in [0x00, 0x16, b'a', b' ', 0xff] {
        assert_eq!(Protocol::from_first_byte(other), None);
    }
    Ok(())
}

struct StubLookup {
    calls: AtomicUsize,
    answer: IpAddr,
}

#[async_trait]
impl NameLookup for StubLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![self.answer])
    }
}

struct CannedTransport {
    reply: Result<&'static str, ()>,
    asked: Mutex<Vec<DecideRequest>>,
}

#[async_trait]
impl DeciderTransport for CannedTransport {
    async fn request(&self, _subject: String, payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError> {
        let request =
            serde_json::from_slice(&payload).map_err(|err| DeciderTransportError::Unavailable(err.to_string()))?;
        self.asked
            .lock()
            .map_err(|_| DeciderTransportError::Unavailable("poisoned".to_string()))?
            .push(request);
        match self.reply {
            Ok(bytes) => Ok(bytes.as_bytes().to_vec()),
            Err(()) => Err(DeciderTransportError::Unavailable("down".to_string())),
        }
    }
}

struct NoPublisher;

#[async_trait]
impl PolicyPublisher for NoPublisher {
    async fn publish(&self, _lease: &LeaseId, _policy: &NetworkPolicy) -> Result<(), String> {
        Ok(())
    }
}

struct Fixture {
    ingress: Ingress,
    lookup: Arc<StubLookup>,
    transport: Arc<CannedTransport>,
    shared: Arc<RwLock<NetworkPolicy>>,
}

const ALLOW_ONCE: &str = r#"{"verdict":"allow","scope":"once"}"#;
const ALLOW_PERSIST: &str = r#"{"verdict":"allow","scope":"persist"}"#;
const DENY_ONCE: &str = r#"{"verdict":"deny","scope":"once"}"#;

fn fixture(
    policy: NetworkPolicy,
    reply: Result<&'static str, ()>,
    answer: IpAddr,
    with_decider: bool,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    let lookup = Arc::new(StubLookup {
        calls: AtomicUsize::new(0),
        answer,
    });
    let transport = Arc::new(CannedTransport {
        reply,
        asked: Mutex::new(Vec::new()),
    });
    let shared = Arc::new(RwLock::new(policy));
    let mut ingress = Ingress::new(
        ProxyToken::new("lease-token-value")?,
        Arc::new(VettingResolver::new(
            lookup.clone(),
            SsrfFloor::system().with_loopback_exception(),
        )),
        IngressLimits::default(),
    );
    if with_decider {
        ingress = ingress.with_decider(Arc::new(Decider::new(
            LeaseId::new("L1"),
            transport.clone(),
            Arc::new(LivePolicyAmender::new(
                LeaseId::new("L1"),
                shared.clone(),
                Arc::new(NoPublisher),
            )),
            DeciderLimits {
                timeout: Duration::from_secs(5),
                max_in_flight: 4,
            },
        )));
    }
    Ok(Fixture {
        ingress,
        lookup,
        transport,
        shared,
    })
}

fn loopback() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

fn user_policy(allow_ranges: &[&str]) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: "listed.test".parse()?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: allow_ranges
            .iter()
            .map(|range| AllowRange::new(*range))
            .collect::<Result<Vec<_>, _>>()?,
        deny_ranges: vec![IpRange::new("10.0.0.0/8")?],
        denied_domains: vec!["blocked.test".parse()?],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn target(host: &str, port: u16) -> Result<ConnectTarget, Box<dyn std::error::Error>> {
    Ok(ConnectTarget::new(Host::new(host)?, Port::new(port)?))
}

async fn admit(f: &Fixture, host: &str, method: DecideMethod) -> Result<Admission, Box<dyn std::error::Error>> {
    let policy = f.shared.read().await.clone();
    Ok(f.ingress.admit(&policy, &target(host, 443)?, method).await)
}

fn denied_reason(admission: Admission) -> Result<EgressViolationReason, Box<dyn std::error::Error>> {
    match admission {
        Admission::Denied { reason, .. } => Ok(reason),
        Admission::Allowed { .. } => Err("allowed".into()),
        Admission::Unresolved(err) => Err(format!("unresolved: {err}").into()),
    }
}

fn asked(f: &Fixture) -> Result<Vec<DecideRequest>, Box<dyn std::error::Error>> {
    Ok(f.transport.asked.lock().map_err(|_| "poisoned")?.clone())
}

#[tokio::test]
async fn an_unlisted_name_is_asked_before_dns_and_allowed_on_a_yes() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Ok(ALLOW_ONCE), loopback(), true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert!(matches!(admission, Admission::Allowed { action: None, .. }));
    let asked = asked(&f)?;
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].host.to_string(), "unlisted.test");
    assert_eq!(asked[0].port.get(), 443);
    assert_eq!(asked[0].method, Some(DecideMethod::Connect));
    assert_eq!(f.lookup.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_no_from_the_decider_denies_before_any_lookup() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Ok(DENY_ONCE), loopback(), true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Socks5).await?;
    assert_eq!(denied_reason(admission)?, EgressViolationReason::DeciderDenied);
    assert_eq!(asked(&f)?[0].method, Some(DecideMethod::Socks5));
    assert_eq!(f.lookup.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn the_http_method_travels_with_the_question() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Ok(ALLOW_ONCE), loopback(), true)?;
    let method = DecideMethod::Http(HttpMethod::new("POST")?);
    admit(&f, "unlisted.test", method.clone()).await?;
    assert_eq!(asked(&f)?[0].method, Some(method));
    Ok(())
}

#[tokio::test]
async fn without_a_decider_an_unlisted_name_is_denied_by_policy() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Ok(ALLOW_ONCE), loopback(), false)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert_eq!(denied_reason(admission)?, EgressViolationReason::PolicyDenied);
    assert_eq!(f.lookup.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_denied_domain_is_never_asked_about() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Ok(ALLOW_ONCE), loopback(), true)?;
    let admission = admit(&f, "blocked.test", DecideMethod::Connect).await?;
    assert_eq!(denied_reason(admission)?, EgressViolationReason::PolicyDenied);
    assert!(asked(&f)?.is_empty());
    assert_eq!(f.lookup.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_listed_name_is_allowed_without_asking() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Err(()), loopback(), true)?;
    let admission = admit(&f, "listed.test", DecideMethod::Connect).await?;
    assert!(matches!(
        admission,
        Admission::Allowed {
            action: Some(DomainRuleAction::PassThrough),
            ..
        }
    ));
    assert!(asked(&f)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_name_an_allow_range_may_cover_is_asked_only_once_resolved_outside_it()
-> Result<(), Box<dyn std::error::Error>> {
    let outside = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
    let f = fixture(user_policy(&["127.0.0.0/8"])?, Ok(ALLOW_ONCE), outside, true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert!(matches!(admission, Admission::Allowed { action: None, .. }));
    assert_eq!(asked(&f)?.len(), 1);
    assert_eq!(f.lookup.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_name_resolving_into_an_allow_range_is_allowed_without_asking() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&["127.0.0.0/8"])?, Err(()), loopback(), true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert!(matches!(admission, Admission::Allowed { .. }));
    assert!(asked(&f)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_name_resolving_into_a_deny_range_is_final_even_if_the_decider_would_allow()
-> Result<(), Box<dyn std::error::Error>> {
    let inside_deny = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
    let f = fixture(user_policy(&["127.0.0.0/8"])?, Ok(ALLOW_PERSIST), inside_deny, true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert!(matches!(admission, Admission::Denied { .. }));
    assert!(asked(&f)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_floor_refuses_a_resolved_address_even_after_a_yes() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254));
    let f = fixture(user_policy(&[])?, Ok(ALLOW_PERSIST), metadata, true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert!(matches!(
        denied_reason(admission)?,
        EgressViolationReason::DestinationAddressDenied(_)
    ));
    Ok(())
}

#[tokio::test]
async fn the_floor_refuses_an_ip_literal_without_asking() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Ok(ALLOW_PERSIST), loopback(), true)?;
    let admission = admit(&f, "169.254.169.254", DecideMethod::Connect).await?;
    assert!(matches!(
        denied_reason(admission)?,
        EgressViolationReason::DestinationAddressDenied(_)
    ));
    assert!(asked(&f)?.is_empty());
    assert_eq!(f.lookup.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn an_unreachable_decider_denies_with_its_own_reason() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Err(()), loopback(), true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert_eq!(denied_reason(admission)?, EgressViolationReason::DeciderUnavailable);
    Ok(())
}

#[tokio::test]
async fn a_malformed_reply_denies_with_its_own_reason() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Ok(r#"{"verdict":"maybe"}"#), loopback(), true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert_eq!(denied_reason(admission)?, EgressViolationReason::DeciderMalformedReply);
    Ok(())
}

#[tokio::test]
async fn a_persisted_allow_amends_the_live_policy_so_the_next_ask_is_unneeded() -> Result<(), Box<dyn std::error::Error>>
{
    let f = fixture(user_policy(&[])?, Ok(ALLOW_PERSIST), loopback(), true)?;
    let first = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert!(matches!(first, Admission::Allowed { .. }));
    let second = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert!(matches!(
        second,
        Admission::Allowed {
            action: Some(DomainRuleAction::PassThrough),
            ..
        }
    ));
    assert_eq!(asked(&f)?.len(), 1);
    Ok(())
}

struct Answers(Vec<IpAddr>);

#[async_trait]
impl NameLookup for Answers {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(self.0.clone())
    }
}

fn name_matched_policy(allowed: &[&str], denied: &[&str]) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: allowed
            .iter()
            .map(|domain| {
                Ok(DomainRule {
                    domain: domain.parse()?,
                    action: DomainRuleAction::PassThrough,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?,
        allow_ranges: allowed
            .iter()
            .filter_map(|domain| domain.parse::<IpAddr>().ok().map(|address| (domain, address)))
            .map(|(domain, address)| AllowRange::new(format!("{domain}/{}", if address.is_ipv4() { 32 } else { 128 })))
            .collect::<Result<Vec<_>, _>>()?,
        deny_ranges: vec![],
        denied_domains: denied
            .iter()
            .map(|domain| domain.parse())
            .collect::<Result<Vec<_>, _>>()?,
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

async fn admit_by_name(
    policy: &NetworkPolicy,
    answers: &[&str],
    host: &str,
    port: u16,
) -> Result<Admission, Box<dyn std::error::Error>> {
    let resolver = VettingResolver::new(
        Arc::new(Answers(
            answers
                .iter()
                .map(|answer| answer.parse())
                .collect::<Result<Vec<IpAddr>, _>>()?,
        )),
        SsrfFloor::system(),
    )
    .with_loopback_names();
    let options = NetworkOptions {
        names_matched_by_name: NamesMatchedByName::Enabled,
        ..NetworkOptions::default()
    };
    Ok(Ingress::open(Arc::new(resolver), IngressLimits::default())
        .with_network_options(Arc::new(options))
        .admit(policy, &target(host, port)?, DecideMethod::Connect)
        .await)
}

fn dialed(admission: Admission) -> Result<IpAddr, Box<dyn std::error::Error>> {
    match admission {
        Admission::Allowed { dest, .. } => Ok(dest.ip()),
        Admission::Denied { reason, .. } => Err(format!("denied: {reason:?}").into()),
        Admission::Unresolved(err) => Err(format!("unresolved: {err}").into()),
    }
}

#[tokio::test]
async fn an_allowed_loopback_name_reaches_loopback() -> Result<(), Box<dyn std::error::Error>> {
    let policy = name_matched_policy(&["localhost"], &[])?;
    let admission = admit_by_name(&policy, &["127.0.0.1"], "localhost", 8080).await?;
    assert_eq!(dialed(admission)?, loopback());
    Ok(())
}

#[tokio::test]
async fn an_allowed_ip_literal_admits_only_a_request_for_that_literal() -> Result<(), Box<dyn std::error::Error>> {
    let policy = name_matched_policy(&["127.0.0.1"], &[])?;
    let literal = admit_by_name(&policy, &["127.0.0.1"], "127.0.0.1", 8080).await?;
    assert_eq!(dialed(literal)?, loopback());
    let by_name = admit_by_name(&policy, &["127.0.0.1"], "localhost", 8080).await?;
    assert_eq!(denied_reason(by_name)?, EgressViolationReason::PolicyDenied);
    Ok(())
}

#[tokio::test]
async fn a_name_skips_a_denied_literal_and_dials_what_is_left() -> Result<(), Box<dyn std::error::Error>> {
    let policy = name_matched_policy(&["localhost", "127.0.0.1"], &["127.0.0.1"])?;
    let literal = admit_by_name(&policy, &["127.0.0.1"], "127.0.0.1", 8080).await?;
    assert_eq!(denied_reason(literal)?, EgressViolationReason::PolicyDenied);
    let by_name = admit_by_name(&policy, &["::1", "127.0.0.1"], "localhost", 8080).await?;
    assert_eq!(dialed(by_name)?, IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
    Ok(())
}

#[tokio::test]
async fn a_name_whose_every_answer_is_a_denied_literal_is_refused_as_deny_listed()
-> Result<(), Box<dyn std::error::Error>> {
    let policy = name_matched_policy(&["localhost", "127.0.0.1"], &["127.0.0.1"])?;
    let admission = admit_by_name(&policy, &["127.0.0.1"], "localhost", 8080).await?;
    let Admission::Denied {
        reason, explanation, ..
    } = admission
    else {
        return Err("allowed".into());
    };
    assert_eq!(
        reason,
        EgressViolationReason::DestinationAddressDenied(AddressDenialClass::DenyListed)
    );
    assert_eq!(
        explanation.ok_or("expected a value")?.as_str(),
        "resolved to a deny-listed address"
    );
    Ok(())
}

#[tokio::test]
async fn a_name_whose_only_answer_carries_a_denied_literal_is_refused_as_deny_listed()
-> Result<(), Box<dyn std::error::Error>> {
    let policy = name_matched_policy(&["myapp.test"], &["8.8.8.8"])?;
    for carried in ["64:ff9b::808:808", "::808:808", "2002:808:808::1"] {
        let admission = admit_by_name(&policy, &[carried], "myapp.test", 8080).await?;
        let Admission::Denied { reason, .. } = admission else {
            return Err(format!("allowed {carried}").into());
        };
        assert_eq!(
            reason,
            EgressViolationReason::DestinationAddressDenied(AddressDenialClass::DenyListed)
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_denied_literal_outranks_the_floor_class_of_the_same_answer() -> Result<(), Box<dyn std::error::Error>> {
    let policy = name_matched_policy(&["myapp.test"], &["127.0.0.1"])?;
    for answer in ["::ffff:127.0.0.1", "64:ff9b::7f00:1"] {
        let admission = admit_by_name(&policy, &[answer], "myapp.test", 8080).await?;
        let Admission::Denied { reason, .. } = admission else {
            return Err(format!("allowed {answer}").into());
        };
        assert_eq!(
            reason,
            EgressViolationReason::DestinationAddressDenied(AddressDenialClass::DenyListed),
            "{answer}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_port_scoped_denied_literal_leaves_the_name_other_ports() -> Result<(), Box<dyn std::error::Error>> {
    let policy = name_matched_policy(&["myapp.test", "127.0.0.1"], &["127.0.0.1:6379"])?;
    let open = admit_by_name(&policy, &["127.0.0.1"], "myapp.test", 3000).await?;
    assert_eq!(dialed(open)?, loopback());
    let closed = admit_by_name(&policy, &["127.0.0.1"], "myapp.test", 6379).await?;
    assert!(matches!(closed, Admission::Denied { .. }));
    Ok(())
}

struct FailingLookup;

#[async_trait]
impl NameLookup for FailingLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Err(io::Error::from(io::ErrorKind::ConnectionRefused))
    }
}

struct StalledLookup;

#[async_trait]
impl NameLookup for StalledLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        std::future::pending().await
    }
}

fn ingress_over(lookup: Arc<dyn NameLookup>) -> Ingress {
    Ingress::open(
        Arc::new(VettingResolver::new(lookup, SsrfFloor::system())),
        IngressLimits {
            resolve_timeout: Duration::from_secs(1),
            ..IngressLimits::default()
        },
    )
}

#[tokio::test]
async fn a_failed_lookup_is_unresolved() -> Result<(), Box<dyn std::error::Error>> {
    let admission = ingress_over(Arc::new(FailingLookup))
        .admit(
            &NetworkPolicy::AllowAll,
            &target("any.test", 443)?,
            DecideMethod::Connect,
        )
        .await;
    assert!(matches!(
        admission,
        Admission::Unresolved(err) if err.kind() == io::ErrorKind::ConnectionRefused
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lookup_that_never_answers_times_out() -> Result<(), Box<dyn std::error::Error>> {
    let admission = ingress_over(Arc::new(StalledLookup))
        .admit(
            &NetworkPolicy::AllowAll,
            &target("any.test", 443)?,
            DecideMethod::Connect,
        )
        .await;
    assert!(matches!(
        admission,
        Admission::Unresolved(err) if err.kind() == io::ErrorKind::TimedOut
    ));
    Ok(())
}

#[tokio::test]
async fn an_address_a_deny_range_covers_is_denied_after_resolution() -> Result<(), Box<dyn std::error::Error>> {
    let covered = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
    let policy = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("127.0.0.0/8")?],
        deny_ranges: vec![IpRange::new("203.0.113.0/24")?],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let f = fixture(policy, Ok(ALLOW_ONCE), covered, false)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert!(matches!(
        admission,
        Admission::Denied {
            dest_ip: Some(ip),
            reason: EgressViolationReason::PolicyDenied,
            ..
        } if ip == covered
    ));
    Ok(())
}

#[tokio::test]
async fn a_no_for_a_name_that_resolved_outside_the_allow_ranges_denies() -> Result<(), Box<dyn std::error::Error>> {
    let outside = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
    let f = fixture(user_policy(&["127.0.0.0/8"])?, Ok(DENY_ONCE), outside, true)?;
    let admission = admit(&f, "unlisted.test", DecideMethod::Connect).await?;
    assert_eq!(denied_reason(admission)?, EgressViolationReason::DeciderDenied);
    Ok(())
}

#[tokio::test]
async fn an_ip_literal_past_the_floor_is_put_to_the_decider() -> Result<(), Box<dyn std::error::Error>> {
    let f = fixture(user_policy(&[])?, Ok(ALLOW_ONCE), loopback(), true)?;
    let admission = admit(&f, "203.0.113.9", DecideMethod::Connect).await?;
    assert!(matches!(admission, Admission::Allowed { .. }));
    assert_eq!(asked(&f)?.len(), 1);
    Ok(())
}

fn serving_context() -> Result<TlsInterceptContext, Box<dyn std::error::Error>> {
    crate::tls_ca::install_default_crypto_provider();
    Ok(TlsInterceptContext::new(
        crate::tls_ca::SandboxCa::generate(crate::tls_ca::PermittedDomains::from_rules(&[])?)?,
        Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        ),
        Arc::new(crate::credential_source::tests::FakeCredentialSource::new()),
        crate::proxy::OidcIdentityContext::unconfigured(crate::oidc_signer::OidcSigner::generate()?, "test")?,
    ))
}

fn tokened_ingress(lookup: Arc<dyn NameLookup>) -> Result<Ingress, Box<dyn std::error::Error>> {
    Ok(Ingress::new(
        ProxyToken::new("lease-token-value")?,
        Arc::new(VettingResolver::new(
            lookup,
            SsrfFloor::system().with_loopback_exception(),
        )),
        IngressLimits::default(),
    ))
}

fn loopback_lookup() -> Arc<dyn NameLookup> {
    Arc::new(Answers(vec![loopback()]))
}

async fn closed_port() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?.port())
}

fn socks5_connect(host: &str, port: u16) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let secret = "lease-token-value";
    let mut bytes = vec![5, 1, 2, 1, 7];
    bytes.extend_from_slice(b"sandbox");
    bytes.push(u8::try_from(secret.len())?);
    bytes.extend_from_slice(secret.as_bytes());
    bytes.extend([5, 1, 0, 3, u8::try_from(host.len())?]);
    bytes.extend_from_slice(host.as_bytes());
    bytes.extend(port.to_be_bytes());
    Ok(bytes)
}

fn http_connect(authority: &str) -> String {
    let credentials = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "sandbox:lease-token-value");
    format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Authorization: Basic {credentials}\r\n\r\n")
}

async fn serve_until_the_client_leaves(
    ingress: &Ingress,
    client_bytes: &[u8],
    keep_reading: bool,
) -> Result<(IngressOutcome, Vec<u8>), Box<dyn std::error::Error>> {
    let (mut client, server) = duplex(64 * 1024);
    client.write_all(client_bytes).await?;
    let context = serving_context()?;
    if !keep_reading {
        drop(client);
        let outcome = ingress.serve(server, &NetworkPolicy::AllowAll, &context).await;
        return Ok((outcome, Vec::new()));
    }
    let outcome = ingress.serve(server, &NetworkPolicy::AllowAll, &context).await;
    let mut answer = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut client, &mut answer).await?;
    Ok((outcome, answer))
}

#[tokio::test]
async fn a_socks5_destination_that_does_not_resolve_is_answered_host_unreachable()
-> Result<(), Box<dyn std::error::Error>> {
    let ingress = tokened_ingress(Arc::new(FailingLookup))?;
    let (outcome, answer) = serve_until_the_client_leaves(&ingress, &socks5_connect("any.test", 443)?, true).await?;
    assert!(matches!(
        outcome,
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::UpstreamConnectFailed(_),
            ..
        }
    ));
    assert!(answer.windows(2).any(|pair| pair == [5, 4]));
    Ok(())
}

#[tokio::test]
async fn a_socks5_destination_that_refuses_is_answered_connection_refused() -> Result<(), Box<dyn std::error::Error>> {
    let ingress = tokened_ingress(loopback_lookup())?;
    let port = closed_port().await?;
    let (outcome, answer) = serve_until_the_client_leaves(&ingress, &socks5_connect("any.test", port)?, true).await?;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }));
    assert!(answer.windows(2).any(|pair| pair == [5, 5]));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_socks5_client_that_never_finishes_its_greeting_times_out() -> Result<(), Box<dyn std::error::Error>> {
    let ingress = tokened_ingress(loopback_lookup())?;
    let (mut client, server) = duplex(1024);
    client.write_all(&[5]).await?;
    let context = serving_context()?;
    let outcome = ingress.serve(server, &NetworkPolicy::AllowAll, &context).await;
    assert!(matches!(
        outcome,
        IngressOutcome::Rejected(IngressRejectionError::HandshakeTimedOut)
    ));
    Ok(())
}

#[tokio::test]
async fn a_socks4_client_that_leaves_early_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let ingress = tokened_ingress(loopback_lookup())?;
    let (outcome, _) = serve_until_the_client_leaves(&ingress, &[4], false).await?;
    assert!(matches!(
        outcome,
        IngressOutcome::Rejected(IngressRejectionError::UnsupportedSocksVersion)
    ));
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_socks5_client_naming_a_malformed_host_is_refused() -> Result<(), Box<dyn std::error::Error>>
{
    let ingress = tokened_ingress(loopback_lookup())?;
    let mut bytes = vec![5, 1, 0, 5, 1, 0, 3, 8];
    bytes.extend_from_slice(b"bad host");
    bytes.extend(443u16.to_be_bytes());
    let (outcome, _) = serve_until_the_client_leaves(&ingress, &bytes, true).await?;
    assert!(matches!(
        outcome,
        IngressOutcome::Rejected(IngressRejectionError::MalformedHost { port: 443, .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_connect_to_a_name_that_does_not_resolve_is_answered_bad_gateway() -> Result<(), Box<dyn std::error::Error>> {
    let ingress = tokened_ingress(Arc::new(FailingLookup))?;
    let (outcome, answer) =
        serve_until_the_client_leaves(&ingress, http_connect("any.test:443").as_bytes(), true).await?;
    assert!(matches!(
        outcome,
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::UpstreamConnectFailed(_),
            ..
        }
    ));
    assert!(String::from_utf8(answer)?.starts_with("HTTP/1.1 502 "));
    Ok(())
}

#[tokio::test]
async fn a_client_gone_before_the_connect_is_established_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let ingress = tokened_ingress(loopback_lookup())?;
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let (outcome, _) =
        serve_until_the_client_leaves(&ingress, http_connect(&format!("any.test:{port}")).as_bytes(), false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn bytes_that_are_not_tls_in_a_connect_are_carried_to_the_destination() -> Result<(), Box<dyn std::error::Error>>
{
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let accepted = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let mut seen = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut seen).await?;
        Ok::<_, io::Error>(seen)
    });
    let ingress = tokened_ingress(loopback_lookup())?;
    let mut bytes = http_connect(&format!("any.test:{port}")).into_bytes();
    bytes.extend_from_slice(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    let (mut client, server) = duplex(64 * 1024);
    client.write_all(&bytes).await?;
    client.shutdown().await?;
    let context = serving_context()?;
    let outcome = ingress.serve(server, &NetworkPolicy::AllowAll, &context).await;
    assert!(matches!(
        outcome,
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Completed { .. },
            ..
        }
    ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), accepted).await???,
        b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn bytes_that_are_not_tls_for_a_destination_that_refuses_are_a_failed_connect()
-> Result<(), Box<dyn std::error::Error>> {
    let ingress = tokened_ingress(loopback_lookup())?;
    let port = closed_port().await?;
    let mut bytes = http_connect(&format!("any.test:{port}")).into_bytes();
    bytes.extend_from_slice(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    let (outcome, _) = serve_until_the_client_leaves(&ingress, &bytes, true).await?;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }));
    Ok(())
}

fn secret_context(hosts: &[&str]) -> Result<TlsInterceptContext, Box<dyn std::error::Error>> {
    let mut registry = trogon_sandbox_protocol::SentinelRegistry::new();
    for host in hosts {
        registry.register(
            trogon_sandbox_contract::SecretName::new("api")?,
            trogon_sandbox_protocol::RealValue::new("real-secret-value".to_string()),
            trogon_sandbox_contract::InjectHosts::new(vec![trogon_sandbox_contract::DomainPattern::new(host)?])?,
        );
    }
    Ok(serving_context()?.with_secrets(crate::SecretSubstitution::new(Arc::new(registry))))
}

async fn serve_in(
    ingress: &Ingress,
    context: &TlsInterceptContext,
    client_bytes: &[u8],
    stays: bool,
) -> Result<IngressOutcome, Box<dyn std::error::Error>> {
    let (mut client, server) = duplex(64 * 1024);
    client.write_all(client_bytes).await?;
    if !stays {
        drop(client);
        return Ok(ingress.serve(server, &NetworkPolicy::AllowAll, context).await);
    }
    client.shutdown().await?;
    Ok(ingress.serve(server, &NetworkPolicy::AllowAll, context).await)
}

const NOT_TLS: &[u8] = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";

async fn live_origin() -> Result<(u16, tokio::task::JoinHandle<Vec<u8>>), Box<dyn std::error::Error>> {
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let seen = tokio::spawn(async move {
        let mut seen = Vec::new();
        if let Ok((mut stream, _)) = upstream.accept().await {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut seen).await;
        }
        seen
    });
    Ok((port, seen))
}

#[tokio::test]
async fn a_terminated_tunnel_that_carries_no_tls_is_spliced_to_the_destination()
-> Result<(), Box<dyn std::error::Error>> {
    let (port, seen) = live_origin().await?;
    let ingress = tokened_ingress(loopback_lookup())?;
    let mut bytes = http_connect(&format!("any.test:{port}")).into_bytes();
    bytes.extend_from_slice(NOT_TLS);
    let outcome = serve_in(&ingress, &secret_context(&["any.test"])?, &bytes, true).await?;
    assert!(
        matches!(
            outcome,
            IngressOutcome::Proxied {
                outcome: ProxyOutcome::Completed { .. },
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), seen).await??, NOT_TLS);
    Ok(())
}

#[tokio::test]
async fn a_terminated_tunnel_that_carries_no_tls_to_a_refusing_destination_fails_to_connect()
-> Result<(), Box<dyn std::error::Error>> {
    let ingress = tokened_ingress(loopback_lookup())?;
    let port = closed_port().await?;
    let mut bytes = http_connect(&format!("any.test:{port}")).into_bytes();
    bytes.extend_from_slice(NOT_TLS);
    let outcome = serve_in(&ingress, &secret_context(&["any.test"])?, &bytes, true).await?;
    assert!(
        matches!(
            outcome,
            IngressOutcome::Proxied {
                outcome: ProxyOutcome::UpstreamConnectFailed(_),
                ..
            }
        ),
        "{outcome:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_client_gone_before_a_terminated_tunnel_is_established_is_rejected() -> Result<(), Box<dyn std::error::Error>>
{
    let (port, _seen) = live_origin().await?;
    let ingress = tokened_ingress(loopback_lookup())?;
    let bytes = http_connect(&format!("any.test:{port}"));
    let outcome = serve_in(&ingress, &secret_context(&["any.test"])?, bytes.as_bytes(), false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    Ok(())
}
