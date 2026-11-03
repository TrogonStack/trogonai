use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

use super::*;

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Buffer {
    type Writer = Buffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn logged(event: &AuditEvent<'_>) -> String {
    let buffer = Buffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || TracingAudit.record(event));
    String::from_utf8(buffer.0.lock().expect("lock").clone()).expect("utf8")
}

fn source() -> PolicySource {
    PolicySource::StaticFile {
        path: PathBuf::from("policy.toml"),
    }
}

#[test]
fn a_permit_names_agent_destination_address_and_source() {
    let agent = AgentId::new("agent-1").expect("agent");
    let destination: Destination = "api.example.com:443".parse().expect("destination");
    let source = source();
    let line = logged(&AuditEvent {
        agent: &agent,
        destination: Some(&destination),
        resolved_address: Some("93.184.216.34".parse().expect("ip")),
        decision: Decision::Permitted,
        snapshot_source: &source,
    });
    for expected in [
        "egress_audit",
        "agent=agent-1",
        "destination=\"api.example.com:443\"",
        "resolved_address=\"93.184.216.34\"",
        "decision=\"permitted\"",
        "fault=\"-\"",
        "snapshot_source=static_file:policy.toml",
    ] {
        assert!(line.contains(expected), "missing {expected} in {line}");
    }
}

#[test]
fn a_refusal_carries_the_fault_code_and_no_address() {
    let agent = AgentId::new("agent-1").expect("agent");
    let destination: Destination = "evil.example:443".parse().expect("destination");
    let source = source();
    let line = logged(&AuditEvent {
        agent: &agent,
        destination: Some(&destination),
        resolved_address: None,
        decision: Decision::Refused(Fault::NoPermit),
        snapshot_source: &source,
    });
    assert!(line.contains("decision=\"refused\""), "{line}");
    assert!(line.contains("fault=\"NO_PERMIT\""), "{line}");
    assert!(line.contains("resolved_address=\"-\""), "{line}");
}

#[test]
fn decision_exposes_its_fault() {
    assert_eq!(Decision::Permitted.fault(), None);
    assert_eq!(Decision::Refused(Fault::SniMismatch).fault(), Some(Fault::SniMismatch));
}
