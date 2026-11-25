use std::path::PathBuf;

use super::*;
use crate::policy_source::PolicySource;
use crate::static_snapshot::StaticSnapshot;

struct Fixed(SnapshotVerdict, PolicySource);

impl Fixed {
    fn new(verdict: SnapshotVerdict) -> Self {
        Self(
            verdict,
            PolicySource::StaticFile {
                path: PathBuf::from("fixed"),
            },
        )
    }
}

impl PolicySnapshot for Fixed {
    fn source(&self) -> &PolicySource {
        &self.1
    }

    fn lookup(&self, _: &AgentId, _: &Destination, _: SystemTime) -> SnapshotVerdict {
        self.0
    }
}

fn agent(raw: &str) -> AgentId {
    AgentId::new(raw).expect("agent")
}

fn destination(raw: &str) -> Destination {
    raw.parse().expect("destination")
}

#[test]
fn maps_each_verdict_to_a_permit_or_a_typed_fault() {
    let who = agent("a");
    let what = destination("example.com:443");
    let now = SystemTime::now();
    assert_eq!(
        admit(&Fixed::new(SnapshotVerdict::Permitted), &who, &what, now),
        Admission::Permit
    );
    assert_eq!(
        admit(&Fixed::new(SnapshotVerdict::NotPermitted), &who, &what, now),
        Admission::Refuse(Fault::NoPermit)
    );
    assert_eq!(
        admit(&Fixed::new(SnapshotVerdict::Unavailable), &who, &what, now),
        Admission::Refuse(Fault::PolicyUnavailable)
    );
}

#[test]
fn works_through_a_trait_object() {
    let snapshot: Box<dyn PolicySnapshot> = Box::new(Fixed::new(SnapshotVerdict::Permitted));
    assert_eq!(
        admit(
            snapshot.as_ref(),
            &agent("a"),
            &destination("example.com:443"),
            SystemTime::now()
        ),
        Admission::Permit
    );
}

fn static_snapshot() -> StaticSnapshot {
    StaticSnapshot::from_toml(
        PolicySource::StaticFile {
            path: PathBuf::from("policy.toml"),
        },
        r#"
[agents.agent-1]
allow = ["api.example.com:443", "10.0.0.5:8080"]
"#,
    )
    .expect("policy")
}

fn run(who: &str, raw: &str) -> Admission {
    admit(&static_snapshot(), &agent(who), &destination(raw), SystemTime::now())
}

#[test]
fn exact_name_and_port_is_admitted() {
    assert_eq!(run("agent-1", "api.example.com:443"), Admission::Permit);
    assert_eq!(run("agent-1", "API.EXAMPLE.COM:443"), Admission::Permit);
}

#[test]
fn a_different_port_or_host_or_agent_has_no_permit() {
    for (who, raw) in [
        ("agent-1", "api.example.com:444"),
        ("agent-1", "evil.api.example.com:443"),
        ("agent-1", "example.com:443"),
        ("agent-2", "api.example.com:443"),
    ] {
        assert_eq!(run(who, raw), Admission::Refuse(Fault::NoPermit), "{who} {raw}");
    }
}

#[test]
fn an_ip_literal_is_admitted_only_when_the_permit_is_that_exact_literal() {
    assert_eq!(run("agent-1", "10.0.0.5:8080"), Admission::Permit);
    assert_eq!(run("agent-1", "10.0.0.5:8081"), Admission::Refuse(Fault::NoPermit));
    assert_eq!(run("agent-1", "10.0.0.6:8080"), Admission::Refuse(Fault::NoPermit));
    assert_eq!(run("agent-1", "[::ffff:10.0.0.5]:8080"), Admission::Permit);
}

#[test]
fn a_name_permit_does_not_admit_the_literal_it_resolves_to() {
    let snapshot = StaticSnapshot::from_toml(
        PolicySource::StaticFile {
            path: PathBuf::from("policy.toml"),
        },
        "[agents.a]\nallow = [\"localhost:443\"]\n",
    )
    .expect("policy");
    assert_eq!(
        admit(&snapshot, &agent("a"), &destination("127.0.0.1:443"), SystemTime::now()),
        Admission::Refuse(Fault::NoPermit)
    );
}
