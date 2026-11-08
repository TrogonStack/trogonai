use std::path::PathBuf;

use super::*;
use crate::policy_snapshot::SnapshotVerdict;
use crate::policy_source::PolicySource;
use crate::static_snapshot::StaticSnapshot;

fn agent() -> AgentId {
    AgentId::new("agent-1").expect("agent")
}

fn port(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).expect("port")
}

fn ip(raw: &str) -> IpAddr {
    raw.parse().expect("address")
}

fn snapshot(allow: &[&str]) -> StaticSnapshot {
    let list = allow
        .iter()
        .map(|entry| format!("\"{entry}\""))
        .collect::<Vec<_>>()
        .join(", ");
    StaticSnapshot::from_toml(
        PolicySource::StaticFile {
            path: PathBuf::from("policy.toml"),
        },
        &format!("[agents.agent-1]\nallow = [{list}]\n"),
    )
    .expect("policy")
}

fn targets(allow: &[&str], candidates: &[&str]) -> Result<Vec<SocketAddr>, Fault> {
    let candidates: Vec<IpAddr> = candidates.iter().map(|raw| ip(raw)).collect();
    dial_targets(&snapshot(allow), &agent(), port(443), &candidates, SystemTime::now())
}

fn sockets(raw: &[&str]) -> Vec<SocketAddr> {
    raw.iter().map(|entry| entry.parse().expect("socket")).collect()
}

#[test]
fn public_addresses_pass_in_resolver_order() {
    assert_eq!(
        targets(&["example.com:443"], &["8.8.8.8", "2606:4700::1111", "1.1.1.1"]),
        Ok(sockets(&["8.8.8.8:443", "[2606:4700::1111]:443", "1.1.1.1:443"]))
    );
}

#[test]
fn duplicates_collapse() {
    assert_eq!(
        targets(&["example.com:443"], &["8.8.8.8", "8.8.8.8", "::ffff:8.8.8.8"]),
        Ok(sockets(&["8.8.8.8:443"]))
    );
}

#[test]
fn internal_addresses_are_refused_for_a_permitted_name() {
    for address in [
        "127.0.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "169.254.169.254",
        "10.0.0.1",
        "172.16.5.5",
        "192.168.0.10",
        "100.64.1.1",
        "fd00::1",
        "fe80::1",
        "0.0.0.0",
        "::",
    ] {
        assert_eq!(
            targets(&["example.com:443"], &[address]),
            Err(Fault::AddressForbidden),
            "{address}"
        );
    }
}

#[test]
fn a_forbidden_answer_next_to_a_public_one_is_skipped_not_dialed() {
    assert_eq!(
        targets(&["example.com:443"], &["127.0.0.1", "169.254.169.254", "8.8.8.8"]),
        Ok(sockets(&["8.8.8.8:443"]))
    );
}

#[test]
fn an_exact_literal_permit_on_the_same_port_allows_the_internal_address() {
    assert_eq!(
        targets(&["internal.example:443", "10.0.0.7:443"], &["10.0.0.7"]),
        Ok(sockets(&["10.0.0.7:443"]))
    );
    assert_eq!(
        targets(&["internal.example:443", "[fd00::7]:443"], &["fd00::7"]),
        Ok(sockets(&["[fd00::7]:443"]))
    );
}

#[test]
fn a_literal_permit_for_another_port_or_address_does_not_help() {
    assert_eq!(
        targets(&["internal.example:443", "10.0.0.7:8443"], &["10.0.0.7"]),
        Err(Fault::AddressForbidden)
    );
    assert_eq!(
        targets(&["internal.example:443", "10.0.0.8:443"], &["10.0.0.7"]),
        Err(Fault::AddressForbidden)
    );
}

#[test]
fn a_mapped_literal_is_judged_as_its_ipv4_form() {
    assert_eq!(
        targets(&["example.com:443", "127.0.0.1:443"], &["::ffff:127.0.0.1"]),
        Ok(sockets(&["127.0.0.1:443"]))
    );
    assert_eq!(
        targets(&["example.com:443"], &["::ffff:127.0.0.1"]),
        Err(Fault::AddressForbidden)
    );
}

#[test]
fn no_candidates_means_nothing_to_dial() {
    assert_eq!(targets(&["example.com:443"], &[]), Err(Fault::AddressForbidden));
}

#[test]
fn an_unavailable_snapshot_is_not_reported_as_a_forbidden_address() {
    struct Unavailable(PolicySource);
    impl PolicySnapshot for Unavailable {
        fn source(&self) -> &PolicySource {
            &self.0
        }
        fn lookup(&self, _: &AgentId, _: &Destination, _: SystemTime) -> SnapshotVerdict {
            SnapshotVerdict::Unavailable
        }
    }
    let snapshot = Unavailable(PolicySource::StaticFile {
        path: PathBuf::from("x"),
    });
    assert_eq!(
        dial_targets(&snapshot, &agent(), port(443), &[ip("127.0.0.1")], SystemTime::now()),
        Err(Fault::PolicyUnavailable)
    );
}
