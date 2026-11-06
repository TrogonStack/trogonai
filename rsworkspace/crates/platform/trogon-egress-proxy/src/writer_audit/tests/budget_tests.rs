use super::*;
use crate::connection_limit::ConnectionLimit;

fn with_limit(sink: MemorySink, limit: ConnectionLimit) -> WriterAudit<MemorySink> {
    let identity = WriterIdentity::new(
        writer(),
        NatsToken::new("egress_proxy").expect("component"),
        AuditName::new("acme").expect("tenant"),
        NatsToken::new("payments").expect("project"),
    );
    let stamper = Stamper::new(SystemClock, UuidV7Generator);
    let writer = AuditWriter::new(sink, identity.clone(), WriterConfig::default(), &stamper);
    WriterAudit::with_limit(writer, identity.tenant().clone(), identity.project().clone(), limit)
}

#[tokio::test]
async fn dropped_callers_cannot_refill_the_actual_audit_worker_budget() {
    let sink = MemorySink::new();
    sink.hold();
    let limit = ConnectionLimit::new(2).expect("bounded limit");
    let audit = with_limit(sink.clone(), limit);
    let agent = AgentId::new("agent-1").expect("agent");
    let destination = "api.example.com:443".parse().expect("destination");
    let policy = source();
    let request = event(&agent, Some(&destination), &policy, Decision::Permitted);
    for _ in 0..limit.get() {
        let mut caller = Box::pin(audit.record(&request));
        assert!(futures::poll!(caller.as_mut()).is_pending());
        drop(caller);
    }
    let excess = tokio::time::timeout(std::time::Duration::from_millis(50), audit.record(&request)).await;
    sink.release();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    audit.run(shutdown).await;
    assert!(
        matches!(excess, Ok(Err(AuditError::Overloaded))),
        "excess request must be refused immediately: {excess:?}"
    );
    assert_eq!(decisions(&sink).len(), limit.get());
}
